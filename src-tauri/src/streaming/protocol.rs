//! 火山引擎大模型流式语音识别二进制 WebSocket 协议（v1）

use anyhow::{anyhow, bail, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::Value;
use std::io::Read;
use std::io::Write;

const PROTOCOL_VERSION: u8 = 0x01;
const HEADER_WORDS: u8 = 0x01; // 4 bytes

const MSG_FULL_CLIENT: u8 = 0x01;
const MSG_AUDIO_ONLY: u8 = 0x02;
const MSG_FULL_SERVER: u8 = 0x09;
const MSG_ERROR: u8 = 0x0f;

const SER_NONE: u8 = 0x00;
const SER_JSON: u8 = 0x01;

const COMP_NONE: u8 = 0x00;
const COMP_GZIP: u8 = 0x01;

const FLAG_NONE: u8 = 0x00;
const FLAG_SEQ_POS: u8 = 0x01;
/// 最后一包（无序号字段，较少使用）
const FLAG_LAST_PACKET: u8 = 0x02;
/// 最后一包，header 后 4 字节为负序号
const FLAG_NEG_WITH_SEQUENCE: u8 = 0x03;

fn header_byte0() -> u8 {
    (PROTOCOL_VERSION << 4) | HEADER_WORDS
}

fn build_header(msg_type: u8, flags: u8, serialization: u8, compression: u8) -> [u8; 4] {
    [
        header_byte0(),
        (msg_type << 4) | (flags & 0x0f),
        (serialization << 4) | (compression & 0x0f),
        0,
    ]
}

fn gzip_compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data)?;
    Ok(enc.finish()?)
}

fn gzip_decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut dec = GzDecoder::new(data);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)?;
    Ok(out)
}

fn frame_with_payload(
    msg_type: u8,
    flags: u8,
    serialization: u8,
    compression: u8,
    sequence: Option<i32>,
    payload: &[u8],
) -> Result<Vec<u8>> {
    let (comp_payload, comp) = if compression == COMP_GZIP {
        (gzip_compress(payload)?, COMP_GZIP)
    } else {
        (payload.to_vec(), COMP_NONE)
    };

    let header = build_header(msg_type, flags, serialization, comp);
    let header_len = 4usize;
    let seq_len = if flags == FLAG_SEQ_POS || flags == FLAG_NEG_WITH_SEQUENCE {
        4
    } else {
        0
    };
    let mut frame = Vec::with_capacity(header_len + seq_len + 4 + comp_payload.len());
    frame.extend_from_slice(&header);
    if seq_len == 4 {
        let seq = sequence.unwrap_or(1);
        frame.extend_from_slice(&seq.to_be_bytes());
    }
    frame.extend_from_slice(&(comp_payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&comp_payload);
    Ok(frame)
}

/// 首包：JSON 参数（gzip），必须带 sequence=1
pub fn encode_full_client_request(json_body: &[u8]) -> Result<Vec<u8>> {
    frame_with_payload(
        MSG_FULL_CLIENT,
        FLAG_SEQ_POS,
        SER_JSON,
        COMP_GZIP,
        Some(1),
        json_body,
    )
}

/// 音频包（gzip 压缩的 pcm_s16le）
/// - 中间包：正序号，从 2 递增
/// - 最后一包：负序号 + FLAG_NEG_WITH_SEQUENCE（与官方 SDK 一致）
pub fn encode_audio_chunk(pcm: &[u8], sequence: i32, is_last: bool) -> Result<Vec<u8>> {
    if is_last {
        let neg_seq = if sequence > 0 { -sequence } else { sequence };
        let compression = if pcm.is_empty() { COMP_NONE } else { COMP_GZIP };
        return frame_with_payload(
            MSG_AUDIO_ONLY,
            FLAG_NEG_WITH_SEQUENCE,
            SER_NONE,
            compression,
            Some(neg_seq),
            pcm,
        );
    }
    frame_with_payload(
        MSG_AUDIO_ONLY,
        FLAG_SEQ_POS,
        SER_NONE,
        COMP_GZIP,
        Some(sequence),
        pcm,
    )
}

pub struct AsrResponse {
    pub text: String,
    pub is_final: bool,
    /// 已确定的文本（utterances 中 definite=true 的部分），不会再变化
    pub definite_text: String,
    /// 临时文本（utterances 中 definite=false 的部分），可能随后被修正
    pub indefinite_text: String,
}

pub fn decode_server_message(data: &[u8]) -> Result<Option<AsrResponse>> {
    if data.len() < 8 {
        bail!("server frame too short");
    }

    let header_words = (data[0] & 0x0f) as usize;
    let header_len = header_words * 4;
    if data.len() < header_len {
        bail!("invalid header length");
    }

    let msg_type = data[1] >> 4;
    let flags = data[1] & 0x0f;
    let compression = data[2] & 0x0f;

    let mut offset = header_len;

    if msg_type == MSG_ERROR {
        if data.len() < offset + 8 {
            bail!("error frame too short");
        }
        let code = read_u32_be(data, offset)?;
        offset += 4;
        let msg_len = read_u32_be(data, offset)? as usize;
        offset += 4;
        let msg_bytes = &data[offset..offset.saturating_add(msg_len).min(data.len())];
        let msg = String::from_utf8_lossy(msg_bytes).into_owned();
        bail!("ASR error {}: {}", code, msg);
    }

    if flags == FLAG_SEQ_POS || flags == FLAG_NEG_WITH_SEQUENCE {
        offset += 4;
    }

    if data.len() < offset + 4 {
        return Ok(None);
    }

    let payload_len = read_u32_be(data, offset)? as usize;
    offset += 4;

    if data.len() < offset + payload_len {
        bail!("truncated payload");
    }

    let payload = &data[offset..offset + payload_len];
    let raw = match compression {
        COMP_GZIP => gzip_decompress(payload)?,
        COMP_NONE => payload.to_vec(),
        _ => bail!("unsupported compression {}", compression),
    };

    if msg_type != MSG_FULL_SERVER {
        return Ok(None);
    }

    let v: Value = serde_json::from_slice(&raw)?;
    if let Some(code) = v.get("code").and_then(|c| c.as_i64()) {
        if code != 20000000 {
            let msg = v
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown");
            bail!("ASR response code {}: {}", code, msg);
        }
    }
    let (definite_text, indefinite_text) = extract_result_segments(&v);

    // text 为完整文本（已确定 + 临时），保持向后兼容
    let text = if indefinite_text.is_empty() {
        definite_text.clone()
    } else if definite_text.is_empty() {
        indefinite_text.clone()
    } else {
        format!("{} {}", definite_text, indefinite_text)
    };

    let is_final = flags == FLAG_NEG_WITH_SEQUENCE || flags == FLAG_LAST_PACKET;

    Ok(Some(AsrResponse {
        text,
        is_final,
        definite_text,
        indefinite_text,
    }))
}

/// 从豆包返回的 utterances 数组中分离已确定和临时文本。
/// 每个 utterance 含 `definite` 字段：true=已确定，false=临时（可能变化）。
/// 返回 (已确定文本, 临时文本)。
fn extract_result_segments(v: &Value) -> (String, String) {
    if let Some(utterances) = v.pointer("/result/utterances").and_then(|u| u.as_array()) {
        if !utterances.is_empty() {
            let mut definite = String::new();
            let mut indefinite = String::new();
            for u in utterances {
                let text = u.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if text.is_empty() {
                    continue;
                }
                let is_definite = u
                    .get("definite")
                    .and_then(|d| d.as_bool())
                    .unwrap_or(false);
                if is_definite {
                    if !definite.is_empty() {
                        definite.push(' ');
                    }
                    definite.push_str(text);
                } else {
                    if !indefinite.is_empty() {
                        indefinite.push(' ');
                    }
                    indefinite.push_str(text);
                }
            }
            if !definite.is_empty() || !indefinite.is_empty() {
                return (definite, indefinite);
            }
        }
    }
    // 回退：没有 utterances 或全部为空，用 result/text，全部当作已确定
    let text = v
        .pointer("/result/text")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    (text, String::new())
}

fn read_u32_be(data: &[u8], offset: usize) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .filter(|&e| e <= data.len())
        .ok_or_else(|| anyhow!("frame too short at offset {}", offset))?;
    Ok(u32::from_be_bytes(data[offset..end].try_into().map_err(|_| {
        anyhow!("invalid u32 at offset {}", offset)
    })?))
}

pub fn build_start_json(
    model_name: &str,
    language: Option<&str>,
    enable_punc: bool,
) -> Result<Vec<u8>> {
    let mut audio = serde_json::json!({
        "format": "pcm",
        "codec": "raw",
        "rate": 16000,
        "bits": 16,
        "channel": 1,
    });
    if let Some(lang) = language.filter(|s| !s.is_empty()) {
        audio["language"] = serde_json::Value::String(lang.to_string());
    }

    let body = serde_json::json!({
        "user": { "uid": "voice2type" },
        "audio": audio,
        "request": {
            "model_name": model_name,
            "enable_itn": true,
            "enable_punc": enable_punc,
            "enable_ddc": true,
            "show_utterances": true,
            "result_type": "full",
            "end_window_size": 800,
        }
    });

    serde_json::to_vec(&body).map_err(|e| anyhow!(e))
}

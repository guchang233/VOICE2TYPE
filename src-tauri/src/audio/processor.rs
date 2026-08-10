use anyhow::Result;

const TARGET_SAMPLE_RATE: u32 = 16_000;

/// 把录音缓冲转换成识别接口更稳的 16kHz / i16 / mono 数据。
///
/// 这里用线性插值，质量比“整数比例抽样”稳定，也不会在 44.1kHz 这类采样率下写错 WAV 头。
pub fn resample_and_convert(input: &[f32], input_rate: u32) -> (Vec<i16>, u32) {
    if input.is_empty() || input_rate == 0 {
        return (Vec::new(), TARGET_SAMPLE_RATE);
    }

    if input_rate == TARGET_SAMPLE_RATE {
        return (float_to_i16(input), TARGET_SAMPLE_RATE);
    }

    let output_len =
        ((input.len() as u64 * TARGET_SAMPLE_RATE as u64) / input_rate as u64).max(1) as usize;
    let step = input_rate as f64 / TARGET_SAMPLE_RATE as f64;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let pos = i as f64 * step;
        let left = pos.floor() as usize;
        let right = (left + 1).min(input.len() - 1);
        let frac = (pos - left as f64) as f32;
        let sample = input[left] * (1.0 - frac) + input[right] * frac;
        output.push(to_i16(sample));
    }

    (output, TARGET_SAMPLE_RATE)
}

/// 将 i16 PCM 样本编码为 WAV 字节流（16-bit / mono / 小端）。
///
/// 手动构建 WAV 头 + 原始字节，避免 `hound` 逐样本 `write_sample` 的函数调用开销。
/// 对于 10 秒 16kHz 音频，从 16 万次函数调用降为一次批量写入。
pub fn encode_wav_memory(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>> {
    let data_size = samples.len() * 2; // 每样本 2 字节
    let total_size = 44 + data_size; // 44 字节头 + 数据

    // 预分配完整缓冲区，一次写入
    let mut buf = Vec::with_capacity(total_size);

    // RIFF 头（12 字节）
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(total_size as u32 - 8).to_le_bytes()); // chunk size
    buf.extend_from_slice(b"WAVE");

    // fmt 子块（24 字节）
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // subchunk1 size
    buf.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // num channels = mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate = sample_rate * channels * bits/8
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align = channels * bits/8
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data 子块头（8 字节）
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&(data_size as u32).to_le_bytes());

    // 批量写入样本字节（小端 i16）
    for &sample in samples {
        buf.extend_from_slice(&sample.to_le_bytes());
    }

    Ok(buf)
}

fn float_to_i16(input: &[f32]) -> Vec<i16> {
    let mut output = Vec::with_capacity(input.len());
    output.extend(input.iter().map(|&sample| to_i16(sample)));
    output
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

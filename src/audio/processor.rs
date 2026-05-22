use anyhow::{Context, Result};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::io::Cursor;

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

pub fn encode_wav_memory(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    // 44 字节 WAV 头 + 16-bit 单声道样本
    let mut cursor = Cursor::new(Vec::with_capacity(44 + samples.len() * 2));
    let mut writer = WavWriter::new(&mut cursor, spec).context("Failed to create WAV writer")?;

    for &sample in samples {
        writer
            .write_sample(sample)
            .context("Failed to write WAV sample")?;
    }

    writer.finalize().context("Failed to finalize WAV writer")?;
    Ok(cursor.into_inner())
}

fn float_to_i16(input: &[f32]) -> Vec<i16> {
    let mut output = Vec::with_capacity(input.len());
    output.extend(input.iter().map(|&sample| to_i16(sample)));
    output
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

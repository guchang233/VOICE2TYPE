use anyhow::{Context, Result};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::io::Cursor;

/// 音频重采样和格式转换
/// 将f32格式的音频转换为i16格式，并将采样率转换为16kHz
pub fn resample_and_convert(input: &[f32], input_rate: u32) -> (Vec<i16>, u32) {
    let target_rate = 16000;
    
    // 如果原始采样率小于等于目标采样率，不做降采样，直接转换
    if input_rate <= target_rate {
        let output: Vec<i16> = input.iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        return (output, input_rate);
    }

    // 计算降采样比率 (简单的整数比率)
    let ratio = (input_rate as f32 / target_rate as f32).round() as usize;
    if ratio <= 1 {
         let output: Vec<i16> = input.iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();
        return (output, input_rate);
    }

    let est_capacity = input.len() / ratio + 1;
    let mut output = Vec::with_capacity(est_capacity);

    // 均值池化 (Average Pooling) 降采样，充当简单的低通滤波
    for chunk in input.chunks(ratio) {
        let sum: f32 = chunk.iter().sum();
        let avg = sum / chunk.len() as f32;
        let sample_i16 = (avg.clamp(-1.0, 1.0) * 32767.0) as i16;
        output.push(sample_i16);
    }
    
    // 计算实际的新采样率
    let actual_new_rate = input_rate / ratio as u32;
    (output, actual_new_rate)
}

/// 内存中编码WAV数据
pub fn encode_wav_memory(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut cursor = Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut cursor, spec)
        .context("Failed to create WAV writer")?;

    for &sample in samples {
        writer.write_sample(sample)
            .context("Failed to write WAV sample")?;
    }

    writer.finalize()
        .context("Failed to finalize WAV writer")?;

    Ok(cursor.into_inner())
}

/// 计算音频能量，用于VAD检测
pub fn calculate_audio_energy(audio: &[f32]) -> f32 {
    if audio.is_empty() {
        return 0.0;
    }
    
    let sum_squared: f32 = audio.iter().map(|&x| x * x).sum();
    sum_squared / audio.len() as f32
}
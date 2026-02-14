use std::collections::VecDeque;

/// VAD（语音活动检测）模块
/// 用于检测音频中的语音活动和非活动时段
#[derive(Debug, Clone)]
pub struct VoiceActivityDetector {
    // 能量阈值，用于判断是否有语音活动
    energy_threshold: f32,
    // 静音帧阈值，用于判断是否为静默
    silence_frame_threshold: usize,
    // 语音帧阈值，用于判断是否为语音
    speech_frame_threshold: usize,
    // 帧大小（样本数）
    frame_size: usize,
    // 采样率
    sample_rate: u32,
    // 能量历史
    energy_history: VecDeque<f32>,
    // 当前状态
    is_speech: bool,
    // 连续语音帧计数
    speech_frame_count: usize,
    // 连续静默帧计数
    silence_frame_count: usize,
}

impl Default for VoiceActivityDetector {
    fn default() -> Self {
        Self {
            energy_threshold: 0.01,
            silence_frame_threshold: 10,
            speech_frame_threshold: 3,
            frame_size: 1024,
            sample_rate: 16000,
            energy_history: VecDeque::with_capacity(30), // 保存30帧的能量历史
            is_speech: false,
            speech_frame_count: 0,
            silence_frame_count: 0,
        }
    }
}

impl VoiceActivityDetector {
    /// 创建新的VAD实例
    pub fn new() -> Self {
        Default::default()
    }

    /// 设置能量阈值
    pub fn set_energy_threshold(&mut self, threshold: f32) {
        self.energy_threshold = threshold;
    }

    /// 设置静音帧阈值
    pub fn set_silence_frame_threshold(&mut self, threshold: usize) {
        self.silence_frame_threshold = threshold;
    }

    /// 设置语音帧阈值
    pub fn set_speech_frame_threshold(&mut self, threshold: usize) {
        self.speech_frame_threshold = threshold;
    }

    /// 设置帧大小
    pub fn set_frame_size(&mut self, frame_size: usize) {
        self.frame_size = frame_size;
    }

    /// 设置采样率
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
    }

    /// 计算音频帧的能量
    pub fn calculate_frame_energy(&self, frame: &[f32]) -> f32 {
        if frame.is_empty() {
            return 0.0;
        }

        let sum: f32 = frame.iter().map(|&x| x * x).sum();
        sum / frame.len() as f32
    }

    /// 处理一帧音频数据
    pub fn process_frame(&mut self, frame: &[f32]) -> bool {
        // 计算当前帧的能量
        let energy = self.calculate_frame_energy(frame);
        
        // 更新能量历史
        self.energy_history.push_back(energy);
        if self.energy_history.len() > 30 {
            self.energy_history.pop_front();
        }

        // 判断当前帧是否为语音
        let is_current_speech = energy > self.energy_threshold;

        if is_current_speech {
            // 重置静默帧计数
            self.silence_frame_count = 0;
            // 增加语音帧计数
            self.speech_frame_count += 1;

            // 如果连续语音帧达到阈值，切换到语音状态
            if !self.is_speech && self.speech_frame_count >= self.speech_frame_threshold {
                self.is_speech = true;
            }
        } else {
            // 重置语音帧计数
            self.speech_frame_count = 0;
            // 增加静默帧计数
            self.silence_frame_count += 1;

            // 如果连续静默帧达到阈值，切换到静默状态
            if self.is_speech && self.silence_frame_count >= self.silence_frame_threshold {
                self.is_speech = false;
            }
        }

        self.is_speech
    }

    /// 检查是否检测到语音活动
    pub fn is_speech(&self) -> bool {
        self.is_speech
    }

    /// 重置VAD状态
    pub fn reset(&mut self) {
        self.energy_history.clear();
        self.is_speech = false;
        self.speech_frame_count = 0;
        self.silence_frame_count = 0;
    }

    /// 获取当前的能量历史
    pub fn energy_history(&self) -> &VecDeque<f32> {
        &self.energy_history
    }
}

/// 音频分片器
/// 用于根据VAD结果将音频分割成合适的片段
#[derive(Debug, Clone)]
pub struct AudioSegmenter {
    vad: VoiceActivityDetector,
    // 最大片段长度（样本数）
    max_segment_length: usize,
    // 当前片段
    current_segment: Vec<f32>,
    // 采样率
    sample_rate: u32,
}

impl Default for AudioSegmenter {
    fn default() -> Self {
        Self {
            vad: VoiceActivityDetector::default(),
            max_segment_length: 16000 * 5, // 5秒音频 @ 16kHz
            current_segment: Vec::new(),
            sample_rate: 16000,
        }
    }
}

impl AudioSegmenter {
    /// 创建新的音频分片器
    pub fn new(sample_rate: u32) -> Self {
        let mut segmenter = Self::default();
        segmenter.sample_rate = sample_rate;
        segmenter.vad.set_sample_rate(sample_rate);
        segmenter
    }

    /// 设置VAD参数
    pub fn set_vad_params(
        &mut self,
        energy_threshold: f32,
        silence_frame_threshold: usize,
        speech_frame_threshold: usize,
        frame_size: usize,
    ) {
        self.vad.set_energy_threshold(energy_threshold);
        self.vad.set_silence_frame_threshold(silence_frame_threshold);
        self.vad.set_speech_frame_threshold(speech_frame_threshold);
        self.vad.set_frame_size(frame_size);
    }

    /// 设置最大片段长度
    pub fn set_max_segment_length(&mut self, max_length_ms: u64) {
        let max_length_samples = (max_length_ms as f32 * self.sample_rate as f32 / 1000.0) as usize;
        self.max_segment_length = max_length_samples;
    }

    /// 处理音频数据
    /// 返回是否需要分片
    pub fn process_audio(&mut self, audio_data: &[f32]) -> bool {
        // 将音频数据分帧处理
        for frame in audio_data.chunks(self.vad.frame_size) {
            // 处理当前帧
            let is_speech = self.vad.process_frame(frame);

            // 将当前帧添加到当前片段
            self.current_segment.extend_from_slice(frame);

            // 检查是否需要分片
            // 1. 如果片段长度超过最大值
            // 2. 如果检测到静默且片段不为空
            if self.current_segment.len() > self.max_segment_length || (!is_speech && !self.current_segment.is_empty()) {
                return true;
            }
        }

        false
    }

    /// 获取当前片段
    pub fn get_current_segment(&self) -> &[f32] {
        &self.current_segment
    }

    /// 取出当前片段
    pub fn take_current_segment(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.current_segment)
    }

    /// 重置分片器
    pub fn reset(&mut self) {
        self.vad.reset();
        self.current_segment.clear();
    }

    /// 检查是否有语音活动
    pub fn is_speech(&self) -> bool {
        self.vad.is_speech()
    }
}

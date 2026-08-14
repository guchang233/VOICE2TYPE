use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const MODEL_TELEAI: &str = "TeleAI/TeleSpeechASR";
pub const MODEL_SENSEVOICE: &str = "FunAudioLLM/SenseVoiceSmall";
pub const MODEL_WHISPER: &str = "whisper-large-v3";
pub const MODEL_LOCAL_WHISPER: &str = "local-whisper";
pub const MODEL_CUSTOM: &str = "custom";

pub const STREAM_MODEL_DOUBAO: &str = "doubao";

pub const STREAMING_ASR_URI: &str = "/api/v3/sauc/bigmodel";
pub const STREAMING_RESOURCE_BIGASR_DURATION: &str = "volc.bigasr.sauc.duration";
pub const STREAMING_RESOURCE_SEEDASR_DURATION: &str = "volc.seedasr.sauc.duration";

pub const STREAMING_POST_AI: &str = "ai";
pub const STREAMING_POST_LOCAL: &str = "local";
pub const STREAMING_POST_NONE: &str = "none";

pub const DICTATION_MODE_BATCH: &str = "batch";
pub const DICTATION_MODE_STREAM: &str = "stream";

// ========== 音频采集质量偏好常量 ==========
/// 音频配置 Profile：预设的一键切换方案
pub const AUDIO_PROFILE_STANDARD: &str = "standard";   // 标准（兼容普通单麦，行为与旧版接近，仅修复U8）
pub const AUDIO_PROFILE_ARRAY_MIC: &str = "array_mic"; // 麦克风阵列优化（强声道下混 + 强制高质量格式）
pub const AUDIO_PROFILE_CUSTOM: &str = "custom";       // 自定义（按下方各自定义字段生效）

/// 下混策略（多声道 → 单声道）
pub const DOWNMIX_AVERAGE: &str = "average";             // 简单平均（旧行为）
pub const DOWNMIX_STRONGEST: &str = "strongest";         // 选 RMS 最大声道（推荐麦克风阵列，避免相位抵消）
pub const DOWNMIX_FIRST_CHANNEL: &str = "first_channel"; // 直接取第 1 声道（部分阵列麦主麦在 ch1）

/// 采样格式偏好
pub const SAMPLE_FMT_AUTO: &str = "auto";   // 按优先级：F32/I16/I32/U16
pub const SAMPLE_FMT_F32: &str = "f32";
pub const SAMPLE_FMT_I16: &str = "i16";
pub const SAMPLE_FMT_I32: &str = "i32";

/// 采样率偏好
pub const SAMPLE_RATE_AUTO: &str = "auto"; // 默认配置或 ≤48kHz
pub const SAMPLE_RATE_16K: &str = "16000";
pub const SAMPLE_RATE_44K: &str = "44100";
pub const SAMPLE_RATE_48K: &str = "48000";

/// 声道数偏好
pub const CHANNELS_AUTO: &str = "auto";
pub const CHANNELS_MONO: &str = "mono";
pub const CHANNELS_STEREO: &str = "stereo";

const SILICONFLOW_TRANSCRIPTIONS_URL: &str = "https://api.siliconflow.cn/v1/audio/transcriptions";
const GROQ_TRANSCRIPTIONS_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";

#[derive(Clone)]
pub struct ConfigManager {
    config: Arc<Mutex<AppConfig>>,
    config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BasicConfig {
    pub model_name: String,
    pub output_language: String,
    pub output_mode: String,
    pub autostart: bool,
    pub hotkey: u32,
    pub show_log: bool,
    pub input_device: String,
    /// 语音输入页面的识别模式：`batch`（整段）或 `stream`（流式）
    pub dictation_mode: String,
    // ===== 音频采集质量偏好 =====
    /// 整体预设 Profile：standard | array_mic | custom
    pub audio_profile: String,
    /// 多声道下混策略：average | strongest | first_channel
    pub audio_downmix: String,
    /// 采样格式偏好：auto | f32 | i16 | i32（始终过滤 U8）
    pub audio_sample_format: String,
    /// 采样率偏好：auto | 16000 | 44100 | 48000
    pub audio_sample_rate: String,
    /// 声道数偏好：auto | mono | stereo
    pub audio_channels: String,
}

impl Default for BasicConfig {
    fn default() -> Self {
        Self {
            model_name: MODEL_SENSEVOICE.to_string(),
            output_language: "auto".to_string(),
            output_mode: "clipboard".to_string(),
            autostart: false,
            hotkey: 0x71,
            show_log: false,
            input_device: String::new(),
            dictation_mode: "batch".to_string(),
            audio_profile: AUDIO_PROFILE_STANDARD.to_string(),
            audio_downmix: DOWNMIX_STRONGEST.to_string(),
            audio_sample_format: SAMPLE_FMT_AUTO.to_string(),
            audio_sample_rate: SAMPLE_RATE_AUTO.to_string(),
            audio_channels: CHANNELS_AUTO.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureConfig {
    pub allow_emoji: bool,
    pub allow_punctuation: bool,
    pub enable_indicator: bool,
    /// 是否启用增强后处理链（PostProcessorChain + TextFormatter）。
    /// false（默认）：使用现有 output::handler::post_process，行为不变。
    /// true：启用新的 LocalCorrector（错别字修正等）+ TextFormatter（中文标点/代码模式）。
    pub enable_post_processor: bool,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            allow_emoji: true,
            allow_punctuation: true,
            enable_indicator: true,
            enable_post_processor: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedConfig {
    pub trigger_mode: String,
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            trigger_mode: "hold".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    pub last_check_time: u64,
    pub ignored_version: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            last_check_time: 0,
            ignored_version: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub siliconflow_api_key: String,
    pub groq_api_key: String,
    pub doubao_api_key: String,
    pub custom_api_key: String,
    pub custom_api_url: String,
    pub custom_model_name: String,
    pub local_whisper_model: String,
    pub local_whisper_dir: String,
    /// 本地 whisper 线程数，0 = 自动取物理核数（上限 8）
    #[serde(default)]
    pub local_whisper_threads: u32,
    /// 贪婪解码（-bs 1 -bo 1），牺牲 CJK 准确率换速度，默认关
    #[serde(default)]
    pub local_whisper_greedy: bool,
    /// 关闭温度回退（-nf），跳过低置信重试，默认关
    #[serde(default)]
    pub local_whisper_no_fallback: bool,
    /// 持久化的 auto 语言检测结果（跨重启复用，避免首调检测开销）
    #[serde(default)]
    pub local_whisper_detected_language: String,
    /// 用户自定义的模型下载目录（为空时使用默认目录）
    pub custom_models_dir: String,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            siliconflow_api_key: String::new(),
            groq_api_key: String::new(),
            doubao_api_key: String::new(),
            custom_api_key: String::new(),
            custom_api_url: SILICONFLOW_TRANSCRIPTIONS_URL.to_string(),
            custom_model_name: "自定义模型".to_string(),
            local_whisper_model: "ggml-tiny.bin".to_string(),
            local_whisper_dir: String::new(),
            local_whisper_threads: 0,
            local_whisper_greedy: false,
            local_whisper_no_fallback: false,
            local_whisper_detected_language: String::new(),
            custom_models_dir: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamingConfig {
    pub hotkey: u32,
    pub trigger_mode: String,
    pub resource_id: String,
    pub model_name: String,
    pub output_language: String,
    pub allow_emoji: bool,
    pub allow_punctuation: bool,
    pub enable_indicator: bool,
    pub post_process_mode: String,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            hotkey: 0x75,
            trigger_mode: "hold".to_string(),
            resource_id: STREAMING_RESOURCE_BIGASR_DURATION.to_string(),
            model_name: "bigmodel".to_string(),
            output_language: String::new(),
            allow_emoji: true,
            allow_punctuation: true,
            enable_indicator: true,
            post_process_mode: STREAMING_POST_NONE.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndicatorConfig {
    pub fade_duration: u64,
    pub error_duration: u64,
    pub success_duration: u64,
}

impl Default for IndicatorConfig {
    fn default() -> Self {
        Self {
            fade_duration: 300,
            error_duration: 5000,
            success_duration: 5000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleConfig {
    pub subtitle_hotkey: u32,
    pub subtitle_font_size: u32,
    pub subtitle_font_color: String,
    pub subtitle_bg_opacity: f32,
    pub subtitle_blur: u32,
    pub subtitle_max_lines: u32,
    pub subtitle_window_x: i32,
    pub subtitle_window_y: i32,
    pub subtitle_window_width: u32,
    pub subtitle_window_height: u32,
    /// 字体族，如 "Microsoft YaHei"、"SimHei"、"Arial"
    pub subtitle_font_family: String,
    /// 字重 100-900
    pub subtitle_font_weight: u32,
    pub subtitle_italic: bool,
    /// 文字对齐：left | center | right
    pub subtitle_text_align: String,
    /// 字间距 px
    pub subtitle_letter_spacing: f32,
    /// 行高倍数，1.4 = 1.4em
    pub subtitle_line_height: f32,
    /// 文字阴影颜色
    pub subtitle_text_shadow_color: String,
    /// 文字阴影强度 0-10
    pub subtitle_text_shadow_strength: u32,
    /// 背景颜色 (RGB)
    pub subtitle_bg_color: String,
    /// 圆角 px
    pub subtitle_border_radius: u32,
    /// 边框颜色
    pub subtitle_border_color: String,
    /// 边框宽度 px
    pub subtitle_border_width: u32,
    /// 内边距 px
    pub subtitle_padding_x: u32,
    pub subtitle_padding_y: u32,
    /// 临时（中间结果）文字颜色
    pub subtitle_interim_color: String,
    /// 临时文字透明度 0-1
    pub subtitle_interim_opacity: f32,
    /// 字幕识别的音频输入设备名（空=使用系统默认设备）
    /// 与 basic.input_device 独立，允许字幕使用不同音源（如立体声混音）
    #[serde(default)]
    pub subtitle_input_device: String,
    /// 字幕识别音源类型："microphone"（麦克风输入设备）| "system"（系统扬声器回放）
    /// 选择 "system" 时使用 WASAPI loopback 捕获电脑正在播放的声音
    #[serde(default = "default_audio_source")]
    pub subtitle_audio_source: String,
    // ===== 元素级显示控制（高度自定义） =====
    /// 显示原文元素
    #[serde(default = "default_true")]
    pub subtitle_show_original: bool,
    /// 显示译文元素（需配合翻译引擎）
    #[serde(default)]
    pub subtitle_show_translation: bool,
    /// 显示说话人标签元素
    #[serde(default)]
    pub subtitle_show_speaker: bool,
    /// 显示时间戳元素
    #[serde(default)]
    pub subtitle_show_timestamp: bool,
    /// 布局方向："vertical"（垂直堆叠）| "horizontal"（水平排列）
    #[serde(default = "default_layout")]
    pub subtitle_layout: String,
    // ===== 译文独立样式 =====
    #[serde(default = "default_translation_font_size")]
    pub subtitle_translation_font_size: u32,
    #[serde(default = "default_white_color")]
    pub subtitle_translation_font_color: String,
    #[serde(default = "default_font_weight_400")]
    pub subtitle_translation_font_weight: u32,
    #[serde(default = "default_translation_opacity")]
    pub subtitle_translation_opacity: f32,
    /// 译文前缀文本，如 "译: "
    #[serde(default)]
    pub subtitle_translation_prefix: String,
    // ===== 说话人样式 =====
    #[serde(default = "default_speaker_color")]
    pub subtitle_speaker_color: String,
    #[serde(default = "default_speaker_font_size")]
    pub subtitle_speaker_font_size: u32,
    /// 说话人前缀，如 "说话人: "
    #[serde(default)]
    pub subtitle_speaker_prefix: String,
    // ===== 时间戳样式 =====
    #[serde(default = "default_timestamp_color")]
    pub subtitle_timestamp_color: String,
    #[serde(default = "default_timestamp_font_size")]
    pub subtitle_timestamp_font_size: u32,
    /// 时间戳格式："HH:MM:SS" | "MM:SS" | "none"
    #[serde(default = "default_timestamp_format")]
    pub subtitle_timestamp_format: String,
    // ===== 翻译配置（预留接口，后续阶段接入引擎） =====
    /// 是否启用翻译
    #[serde(default)]
    pub subtitle_translation_enabled: bool,
    /// 翻译目标语言代码：zh / en / ja / ko / fr / de / ...
    #[serde(default = "default_translation_target_lang")]
    pub subtitle_translation_target_lang: String,
    /// 翻译引擎："none" | "llm" | "aliyun" | "deepl"
    #[serde(default = "default_translation_engine")]
    pub subtitle_translation_engine: String,
    // ===== 自定义元素系统（高度自定义） =====
    /// 自定义元素列表（用户可添加任意数量的文本/分隔元素）
    #[serde(default)]
    pub subtitle_custom_elements: Vec<SubtitleCustomElement>,
    /// 元素显示顺序：固定元素 ID 为 "original"/"translation"/"speaker"/"timestamp"，
    /// 自定义元素使用其 id。未列出的可见元素追加在末尾。
    #[serde(default = "default_element_order")]
    pub subtitle_element_order: Vec<String>,
    /// 当前预设模板："clean" | "bilingual" | "meeting" | "live" | "custom"
    #[serde(default = "default_preset")]
    pub subtitle_preset: String,
    /// 多场景字幕窗口列表。至少包含 id="default" 的主场景，
    /// 其余场景运行时动态创建独立字幕窗口（每窗口独立样式/位置/置顶/穿透）。
    #[serde(default)]
    pub subtitle_scenes: Vec<SubtitleSceneConfig>,
    /// 同声传译 LLM（OpenAI 兼容 chat/completions）共享接口配置。
    /// API Key 留空时回退使用「LLM 智能校对」的配置。
    #[serde(default)]
    pub subtitle_translation_llm_api_url: String,
    #[serde(default)]
    pub subtitle_translation_llm_api_key: String,
    #[serde(default)]
    pub subtitle_translation_llm_model: String,
}

/// 字幕自定义元素（用户可添加的文本/分隔元素）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleCustomElement {
    /// 唯一 ID
    pub id: String,
    /// 元素类型："text" | "divider" | "spacer"
    #[serde(default = "default_element_type")]
    pub element_type: String,
    /// 显示名称（用于设置面板）
    pub label: String,
    /// 文本内容（text 类型），支持占位符 {time} {date}
    pub content: String,
    /// 是否可见
    #[serde(default = "default_true")]
    pub visible: bool,
    /// 颜色
    #[serde(default = "default_white_color")]
    pub color: String,
    /// 字号 px
    #[serde(default = "default_custom_font_size")]
    pub font_size: u32,
    /// 字重 100-900
    #[serde(default = "default_font_weight_400")]
    pub font_weight: u32,
    /// 透明度 0-1
    #[serde(default = "default_custom_opacity")]
    pub opacity: f32,
    /// 前缀文本
    #[serde(default)]
    pub prefix: String,
    /// 对齐："left" | "center" | "right"
    #[serde(default = "default_align_center")]
    pub align: String,
}

impl Default for SubtitleCustomElement {
    fn default() -> Self {
        Self {
            id: String::new(),
            element_type: "text".to_string(),
            label: "自定义文本".to_string(),
            content: String::new(),
            visible: true,
            color: "#ffffff".to_string(),
            font_size: 18,
            font_weight: 400,
            opacity: 0.9,
            prefix: String::new(),
            align: "center".to_string(),
        }
    }
}

/// 字幕场景的窗口控制配置（位置/尺寸/置顶/穿透/OBS 模式）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleSceneWindowConfig {
    /// 窗口位置 x（-1 = 未设置，使用系统默认位置）
    pub x: i32,
    /// 窗口位置 y（-1 = 未设置，使用系统默认位置）
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub always_on_top: bool,
    pub click_through: bool,
    pub obs_mode: bool,
}

impl Default for SubtitleSceneWindowConfig {
    fn default() -> Self {
        Self {
            x: -1,
            y: -1,
            width: 1200,
            height: 120,
            always_on_top: true,
            click_through: false,
            obs_mode: false,
        }
    }
}

/// 字幕场景的完整样式（与 SubtitleConfig 中的扁平样式字段一一对应）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleStyle {
    pub font_family: String,
    pub font_size: u32,
    pub font_color: String,
    pub font_weight: u32,
    pub italic: bool,
    pub text_align: String,
    pub letter_spacing: f32,
    pub line_height: f32,
    pub text_shadow_color: String,
    pub text_shadow_strength: u32,
    pub bg_color: String,
    pub bg_opacity: f32,
    pub blur: u32,
    pub border_radius: u32,
    pub border_color: String,
    pub border_width: u32,
    pub padding_x: u32,
    pub padding_y: u32,
    pub max_lines: u32,
    pub interim_color: String,
    pub interim_opacity: f32,
    pub show_original: bool,
    pub show_translation: bool,
    pub show_speaker: bool,
    pub show_timestamp: bool,
    pub layout: String,
    pub translation_font_size: u32,
    pub translation_font_color: String,
    pub translation_font_weight: u32,
    pub translation_opacity: f32,
    pub translation_prefix: String,
    pub speaker_color: String,
    pub speaker_font_size: u32,
    pub speaker_prefix: String,
    pub timestamp_color: String,
    pub timestamp_font_size: u32,
    pub timestamp_format: String,
    pub custom_elements: Vec<SubtitleCustomElement>,
    pub element_order: Vec<String>,
    pub preset: String,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        let base = SubtitleConfig::default();
        Self {
            font_family: base.subtitle_font_family,
            font_size: base.subtitle_font_size,
            font_color: base.subtitle_font_color,
            font_weight: base.subtitle_font_weight,
            italic: base.subtitle_italic,
            text_align: base.subtitle_text_align,
            letter_spacing: base.subtitle_letter_spacing,
            line_height: base.subtitle_line_height,
            text_shadow_color: base.subtitle_text_shadow_color,
            text_shadow_strength: base.subtitle_text_shadow_strength,
            bg_color: base.subtitle_bg_color,
            bg_opacity: base.subtitle_bg_opacity,
            blur: base.subtitle_blur,
            border_radius: base.subtitle_border_radius,
            border_color: base.subtitle_border_color,
            border_width: base.subtitle_border_width,
            padding_x: base.subtitle_padding_x,
            padding_y: base.subtitle_padding_y,
            max_lines: base.subtitle_max_lines,
            interim_color: base.subtitle_interim_color,
            interim_opacity: base.subtitle_interim_opacity,
            show_original: base.subtitle_show_original,
            show_translation: base.subtitle_show_translation,
            show_speaker: base.subtitle_show_speaker,
            show_timestamp: base.subtitle_show_timestamp,
            layout: base.subtitle_layout,
            translation_font_size: base.subtitle_translation_font_size,
            translation_font_color: base.subtitle_translation_font_color,
            translation_font_weight: base.subtitle_translation_font_weight,
            translation_opacity: base.subtitle_translation_opacity,
            translation_prefix: base.subtitle_translation_prefix,
            speaker_color: base.subtitle_speaker_color,
            speaker_font_size: base.subtitle_speaker_font_size,
            speaker_prefix: base.subtitle_speaker_prefix,
            timestamp_color: base.subtitle_timestamp_color,
            timestamp_font_size: base.subtitle_timestamp_font_size,
            timestamp_format: base.subtitle_timestamp_format,
            custom_elements: Vec::new(),
            element_order: default_element_order(),
            preset: "clean".to_string(),
        }
    }
}

/// 同声传译配置（每场景独立：引擎/目标语言/是否翻译中间结果）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleTranslationConfig {
    /// 翻译引擎："none"（关闭） | "llm"（OpenAI 兼容 LLM）
    pub engine: String,
    /// 目标语言（自然语言描述：中文/英文/日文/韩文/法文/德文/西班牙文/俄文）
    pub target_lang: String,
    /// 是否实时翻译临时（中间）识别结果，实现"同声"预览效果
    pub interim: bool,
}

impl Default for SubtitleTranslationConfig {
    fn default() -> Self {
        Self {
            engine: "none".to_string(),
            target_lang: "英文".to_string(),
            interim: true,
        }
    }
}

/// 一个字幕场景 = 一个独立的自定义字幕窗口
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleSceneConfig {
    /// 唯一 ID："default" 为主场景（静态 subtitle 窗口），其余为动态窗口
    pub id: String,
    /// 场景名称（设置页展示）
    pub name: String,
    /// 是否启用（禁用则不创建窗口、不接收字幕）
    pub enabled: bool,
    pub window: SubtitleSceneWindowConfig,
    pub style: SubtitleStyle,
    pub translation: SubtitleTranslationConfig,
}

impl Default for SubtitleSceneConfig {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            name: "默认字幕".to_string(),
            enabled: true,
            window: SubtitleSceneWindowConfig::default(),
            style: SubtitleStyle::default(),
            translation: SubtitleTranslationConfig::default(),
        }
    }
}

impl SubtitleSceneConfig {
    /// 从旧的扁平字段构建默认场景（老配置迁移用）
    pub fn from_legacy(sub: &SubtitleConfig) -> Self {
        Self {
            id: "default".to_string(),
            name: "默认字幕".to_string(),
            enabled: true,
            window: SubtitleSceneWindowConfig {
                x: sub.subtitle_window_x,
                y: sub.subtitle_window_y,
                width: sub.subtitle_window_width,
                height: sub.subtitle_window_height,
                always_on_top: true,
                click_through: false,
                obs_mode: false,
            },
            style: SubtitleStyle {
                font_family: sub.subtitle_font_family.clone(),
                font_size: sub.subtitle_font_size,
                font_color: sub.subtitle_font_color.clone(),
                font_weight: sub.subtitle_font_weight,
                italic: sub.subtitle_italic,
                text_align: sub.subtitle_text_align.clone(),
                letter_spacing: sub.subtitle_letter_spacing,
                line_height: sub.subtitle_line_height,
                text_shadow_color: sub.subtitle_text_shadow_color.clone(),
                text_shadow_strength: sub.subtitle_text_shadow_strength,
                bg_color: sub.subtitle_bg_color.clone(),
                bg_opacity: sub.subtitle_bg_opacity,
                blur: sub.subtitle_blur,
                border_radius: sub.subtitle_border_radius,
                border_color: sub.subtitle_border_color.clone(),
                border_width: sub.subtitle_border_width,
                padding_x: sub.subtitle_padding_x,
                padding_y: sub.subtitle_padding_y,
                max_lines: sub.subtitle_max_lines,
                interim_color: sub.subtitle_interim_color.clone(),
                interim_opacity: sub.subtitle_interim_opacity,
                show_original: sub.subtitle_show_original,
                show_translation: sub.subtitle_show_translation,
                show_speaker: sub.subtitle_show_speaker,
                show_timestamp: sub.subtitle_show_timestamp,
                layout: sub.subtitle_layout.clone(),
                translation_font_size: sub.subtitle_translation_font_size,
                translation_font_color: sub.subtitle_translation_font_color.clone(),
                translation_font_weight: sub.subtitle_translation_font_weight,
                translation_opacity: sub.subtitle_translation_opacity,
                translation_prefix: sub.subtitle_translation_prefix.clone(),
                speaker_color: sub.subtitle_speaker_color.clone(),
                speaker_font_size: sub.subtitle_speaker_font_size,
                speaker_prefix: sub.subtitle_speaker_prefix.clone(),
                timestamp_color: sub.subtitle_timestamp_color.clone(),
                timestamp_font_size: sub.subtitle_timestamp_font_size,
                timestamp_format: sub.subtitle_timestamp_format.clone(),
                custom_elements: sub.subtitle_custom_elements.clone(),
                element_order: sub.subtitle_element_order.clone(),
                preset: sub.subtitle_preset.clone(),
            },
            translation: SubtitleTranslationConfig {
                engine: sub.subtitle_translation_engine.clone(),
                target_lang: sub.subtitle_translation_target_lang.clone(),
                interim: true,
            },
        }
    }

    /// 校验并归一化场景配置（非法值回退默认）
    pub fn normalize(&mut self) {
        if self.id.trim().is_empty() {
            self.id = "default".to_string();
        }
        if self.name.trim().is_empty() {
            self.name = "字幕场景".to_string();
        }
        if self.window.width == 0 {
            self.window.width = 1200;
        }
        if self.window.height == 0 {
            self.window.height = 120;
        }
        self.window.width = self.window.width.clamp(200, 3840);
        self.window.height = self.window.height.clamp(40, 2160);

        let s = &mut self.style;
        s.bg_opacity = s.bg_opacity.clamp(0.0, 1.0);
        s.interim_opacity = s.interim_opacity.clamp(0.0, 1.0);
        s.font_weight = (s.font_weight.clamp(100, 900) / 100) * 100;
        s.text_shadow_strength = s.text_shadow_strength.min(10);
        s.letter_spacing = s.letter_spacing.clamp(-5.0, 20.0);
        s.line_height = s.line_height.clamp(0.8, 3.0);
        s.text_align = match s.text_align.as_str() {
            "left" | "center" | "right" => s.text_align.clone(),
            _ => "center".to_string(),
        };
        s.layout = match s.layout.as_str() {
            "vertical" | "horizontal" => s.layout.clone(),
            _ => "vertical".to_string(),
        };
        s.translation_opacity = s.translation_opacity.clamp(0.0, 1.0);
        s.translation_font_weight = (s.translation_font_weight.clamp(100, 900) / 100) * 100;
        s.translation_font_size = s.translation_font_size.clamp(8, 96);
        s.speaker_font_size = s.speaker_font_size.clamp(8, 48);
        s.timestamp_font_size = s.timestamp_font_size.clamp(8, 48);
        s.timestamp_format = match s.timestamp_format.as_str() {
            "HH:MM:SS" | "MM:SS" | "none" => s.timestamp_format.clone(),
            _ => "HH:MM:SS".to_string(),
        };
        s.preset = match s.preset.as_str() {
            "clean" | "bilingual" | "meeting" | "live" | "custom" => s.preset.clone(),
            _ => "clean".to_string(),
        };
        for fixed in &["speaker", "original", "translation", "timestamp"] {
            if !s.element_order.iter().any(|e| e == fixed) {
                s.element_order.push((*fixed).to_string());
            }
        }
        for (i, el) in s.custom_elements.iter_mut().enumerate() {
            el.element_type = match el.element_type.as_str() {
                "text" | "divider" | "spacer" => el.element_type.clone(),
                _ => "text".to_string(),
            };
            el.align = match el.align.as_str() {
                "left" | "center" | "right" => el.align.clone(),
                _ => "center".to_string(),
            };
            el.font_weight = (el.font_weight.clamp(100, 900) / 100) * 100;
            el.font_size = el.font_size.clamp(8, 96);
            el.opacity = el.opacity.clamp(0.0, 1.0);
            if el.id.is_empty() {
                el.id = format!("custom_{}", i);
            }
        }

        let t = &mut self.translation;
        t.engine = match t.engine.as_str() {
            "none" | "llm" => t.engine.clone(),
            _ => "none".to_string(),
        };
        if t.target_lang.trim().is_empty() {
            t.target_lang = "英文".to_string();
        }
    }
}

fn default_true() -> bool { true }
fn default_audio_source() -> String { "microphone".to_string() }
fn default_layout() -> String { "vertical".to_string() }fn default_translation_font_size() -> u32 { 24 }
fn default_white_color() -> String { "#ffffff".to_string() }
fn default_font_weight_400() -> u32 { 400 }
fn default_translation_opacity() -> f32 { 0.85 }
fn default_speaker_color() -> String { "#818cf8".to_string() }
fn default_speaker_font_size() -> u32 { 16 }
fn default_timestamp_color() -> String { "#a1a1aa".to_string() }
fn default_timestamp_font_size() -> u32 { 14 }
fn default_timestamp_format() -> String { "HH:MM:SS".to_string() }
fn default_translation_target_lang() -> String { "en".to_string() }
fn default_translation_engine() -> String { "none".to_string() }
fn default_element_order() -> Vec<String> {
    vec![
        "speaker".to_string(),
        "original".to_string(),
        "translation".to_string(),
        "timestamp".to_string(),
    ]
}
fn default_preset() -> String { "clean".to_string() }
fn default_custom_font_size() -> u32 { 18 }
fn default_custom_opacity() -> f32 { 0.9 }
fn default_align_center() -> String { "center".to_string() }
fn default_element_type() -> String { "text".to_string() }

impl Default for SubtitleConfig {
    fn default() -> Self {
        Self {
            subtitle_hotkey: 0x76,
            subtitle_font_size: 32,
            subtitle_font_color: "#ffffff".to_string(),
            subtitle_bg_opacity: 0.6,
            subtitle_blur: 20,
            subtitle_max_lines: 3,
            subtitle_window_x: -1,
            subtitle_window_y: -1,
            subtitle_window_width: 1200,
            subtitle_window_height: 120,
            subtitle_font_family: "Microsoft YaHei".to_string(),
            subtitle_font_weight: 400,
            subtitle_italic: false,
            subtitle_text_align: "center".to_string(),
            subtitle_letter_spacing: 0.0,
            subtitle_line_height: 1.4,
            subtitle_text_shadow_color: "#000000".to_string(),
            subtitle_text_shadow_strength: 4,
            subtitle_bg_color: "#000000".to_string(),
            subtitle_border_radius: 12,
            subtitle_border_color: "#ffffff".to_string(),
            subtitle_border_width: 0,
            subtitle_padding_x: 24,
            subtitle_padding_y: 12,
            subtitle_interim_color: "#ffffff".to_string(),
            subtitle_interim_opacity: 0.7,
            subtitle_input_device: String::new(),
            subtitle_audio_source: "microphone".to_string(),
            // 元素级显示控制
            subtitle_show_original: true,
            subtitle_show_translation: false,
            subtitle_show_speaker: false,
            subtitle_show_timestamp: false,
            subtitle_layout: "vertical".to_string(),
            // 译文样式
            subtitle_translation_font_size: 24,
            subtitle_translation_font_color: "#ffffff".to_string(),
            subtitle_translation_font_weight: 400,
            subtitle_translation_opacity: 0.85,
            subtitle_translation_prefix: String::new(),
            // 说话人样式
            subtitle_speaker_color: "#818cf8".to_string(),
            subtitle_speaker_font_size: 16,
            subtitle_speaker_prefix: String::new(),
            // 时间戳样式
            subtitle_timestamp_color: "#a1a1aa".to_string(),
            subtitle_timestamp_font_size: 14,
            subtitle_timestamp_format: "HH:MM:SS".to_string(),
            // 翻译配置
            subtitle_translation_enabled: false,
            subtitle_translation_target_lang: "en".to_string(),
            subtitle_translation_engine: "none".to_string(),
            // 自定义元素系统
            subtitle_custom_elements: Vec::new(),
            subtitle_element_order: default_element_order(),
            subtitle_preset: "clean".to_string(),
            // 多场景与同声传译
            subtitle_scenes: Vec::new(),
            subtitle_translation_llm_api_url: String::new(),
            subtitle_translation_llm_api_key: String::new(),
            subtitle_translation_llm_model: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VadConfig {
    pub vad_sensitivity: f32,
    pub vad_silence_duration_ms: u32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            vad_sensitivity: 0.5,
            vad_silence_duration_ms: 800,
        }
    }
}

/// LLM 智能后处理配置。
///
/// 允许用户自行配置 OpenAI 兼容的 Chat 接口，对 ASR 识别文本做智能校对：
/// - 中文同音错字修正
/// - 语序与标点优化
/// - 上下文相关纠错
///
/// 默认禁用。启用后需配置 `api_url` / `api_key` / `model`。
/// 默认指向硅基流动（SiliconFlow）免费的 Qwen2.5-7B-Instruct，用户也可改为 OpenAI / DeepSeek / KIMI 等。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmPostProcessConfig {
    /// 是否启用 LLM 智能校对。
    pub enable: bool,
    /// OpenAI 兼容的 chat/completions 端点。
    pub api_url: String,
    /// API Key（Bearer 鉴权）。
    pub api_key: String,
    /// 模型名称，如 `Qwen/Qwen2.5-7B-Instruct`、`gpt-4o-mini`、`moonshot-v1-8k`。
    pub model: String,
    /// 自定义系统提示词。留空则使用内置默认 prompt。
    pub system_prompt: String,
}

impl Default for LlmPostProcessConfig {
    fn default() -> Self {
        Self {
            enable: false,
            // 硅基流动（SiliconFlow）免费模型
            api_url: "https://api.siliconflow.cn/v1/chat/completions".to_string(),
            api_key: String::new(),
            model: "Qwen/Qwen2.5-7B-Instruct".to_string(),
            system_prompt: String::new(),
        }
    }
}

/// Fish Audio 文本转语音（TTS）配置。
///
/// 调用 Fish Audio 的 TTS 接口，将文本转为语音。默认使用免费层 `s2.1-pro-free` 模型，
/// 支持官方音色库（reference_id）、参数调节（语速/音量/温度等）、试听与下载。
///
/// API 文档：https://docs.fish.audio
/// - TTS: POST https://api.fish.audio/v1/tts （model 通过请求头传递）
/// - 音色库: GET https://api.fish.audio/model
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    /// Fish Audio API Key（Bearer 鉴权）。在 https://fish.audio/app/api-keys 获取。
    pub fish_api_key: String,
    /// TTS 模型，通过请求头 `model` 传递。可选：s2.1-pro-free（免费，默认）/ s2.1-pro / s2-pro / s1
    pub model: String,
    /// 选中的音色 ID（reference_id），对应官方音色库或自建音色。留空则使用模型默认音色。
    pub reference_id: String,
    /// 选中的音色标题（仅前端展示用，便于回显）
    pub reference_title: String,
    /// 输出音频格式：mp3 / wav / pcm / opus
    pub format: String,
    /// 语速倍率 0.5–2.0，1.0 = 正常
    pub speed: f32,
    /// 音量调整（dB），正值更大，负值更小
    pub volume: f32,
    /// 延迟-质量权衡：normal / balanced / low
    pub latency: String,
    /// 文本分段大小 100–300
    pub chunk_length: u32,
    /// 是否归一化数字/日期/缩写
    pub normalize: bool,
    /// 采样多样性 0–1
    pub temperature: f32,
    /// nucleus sampling 0–1
    pub top_p: f32,
    /// MP3 比特率 64/128/192（仅 format=mp3 生效）
    pub mp3_bitrate: u32,
    /// 采样率（Hz），0 = 使用格式默认值
    pub sample_rate: u32,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            fish_api_key: String::new(),
            model: "s2.1-pro-free".to_string(),
            reference_id: String::new(),
            reference_title: String::new(),
            format: "mp3".to_string(),
            speed: 1.0,
            volume: 0.0,
            latency: "normal".to_string(),
            chunk_length: 200,
            normalize: true,
            temperature: 0.7,
            top_p: 0.7,
            mp3_bitrate: 128,
            sample_rate: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSelectionConfig {
    pub batch_model: String,
    pub stream_model: String,
    pub subtitle_model: String,
}

impl Default for ModelSelectionConfig {
    fn default() -> Self {
        Self {
            batch_model: MODEL_SENSEVOICE.to_string(),
            stream_model: STREAM_MODEL_DOUBAO.to_string(),
            subtitle_model: STREAM_MODEL_DOUBAO.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub basic: BasicConfig,
    pub features: FeatureConfig,
    pub advanced: AdvancedConfig,
    pub update: UpdateConfig,
    pub model: ModelConfig,
    pub streaming: StreamingConfig,
    pub indicator: IndicatorConfig,
    pub subtitle: SubtitleConfig,
    pub vad: VadConfig,
    pub model_selection: ModelSelectionConfig,
    pub llm_post: LlmPostProcessConfig,
    pub tts: TtsConfig,
    pub theme: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_log: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_emoji: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_punctuation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_indicator: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignored_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub siliconflow_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groq_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_api_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicator_fade_duration: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicator_error_duration: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicator_success_duration: Option<u64>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            basic: BasicConfig::default(),
            features: FeatureConfig::default(),
            advanced: AdvancedConfig::default(),
            update: UpdateConfig::default(),
            model: ModelConfig::default(),
            streaming: StreamingConfig::default(),
            indicator: IndicatorConfig::default(),
            subtitle: SubtitleConfig::default(),
            vad: VadConfig::default(),
            model_selection: ModelSelectionConfig::default(),
            llm_post: LlmPostProcessConfig::default(),
            tts: TtsConfig::default(),
            theme: "auto".to_string(),
            model_name: None,
            output_language: None,
            output_mode: None,
            autostart: None,
            hotkey: None,
            show_log: None,
            allow_emoji: None,
            allow_punctuation: None,
            enable_indicator: None,
            trigger_mode: None,
            last_check_time: None,
            ignored_version: None,
            siliconflow_api_key: None,
            groq_api_key: None,
            custom_api_key: None,
            custom_api_url: None,
            custom_model_name: None,
            indicator_fade_duration: None,
            indicator_error_duration: None,
            indicator_success_duration: None,
        }
    }
}

impl AppConfig {
    pub fn initialize(&mut self) {
        if let Some(value) = self.model_name.take() {
            self.basic.model_name = value;
        }
        if let Some(value) = self.output_language.take() {
            self.basic.output_language = value;
        }
        if let Some(value) = self.output_mode.take() {
            self.basic.output_mode = value;
        }
        if let Some(value) = self.autostart.take() {
            self.basic.autostart = value;
        }
        if let Some(value) = self.hotkey.take() {
            self.basic.hotkey = value;
        }
        if let Some(value) = self.show_log.take() {
            self.basic.show_log = value;
        }
        if let Some(value) = self.allow_emoji.take() {
            self.features.allow_emoji = value;
        }
        if let Some(value) = self.allow_punctuation.take() {
            self.features.allow_punctuation = value;
        }
        if let Some(value) = self.enable_indicator.take() {
            self.features.enable_indicator = value;
        }
        if let Some(value) = self.trigger_mode.take() {
            self.advanced.trigger_mode = value;
        }
        if let Some(value) = self.last_check_time.take() {
            self.update.last_check_time = value;
        }
        if let Some(value) = self.ignored_version.take() {
            self.update.ignored_version = value;
        }
        if let Some(value) = self.siliconflow_api_key.take() {
            self.model.siliconflow_api_key = value;
        }
        if let Some(value) = self.groq_api_key.take() {
            self.model.groq_api_key = value;
        }
        if let Some(value) = self.custom_api_key.take() {
            self.model.custom_api_key = value;
        }
        if let Some(value) = self.custom_api_url.take() {
            self.model.custom_api_url = value;
        }
        if let Some(value) = self.custom_model_name.take() {
            self.model.custom_model_name = value;
        }
        if let Some(value) = self.indicator_fade_duration.take() {
            self.indicator.fade_duration = value;
        }
        if let Some(value) = self.indicator_error_duration.take() {
            self.indicator.error_duration = value;
        }
        if let Some(value) = self.indicator_success_duration.take() {
            self.indicator.success_duration = value;
        }
        self.streaming.post_process_mode =
            Self::normalize_streaming_post_process_mode(&self.streaming.post_process_mode);
        self.basic.dictation_mode = Self::normalize_dictation_mode(&self.basic.dictation_mode);
        self.subtitle.subtitle_bg_opacity = self.subtitle.subtitle_bg_opacity.clamp(0.0, 1.0);
        self.subtitle.subtitle_interim_opacity = self.subtitle.subtitle_interim_opacity.clamp(0.0, 1.0);
        self.subtitle.subtitle_font_weight = self.subtitle.subtitle_font_weight.clamp(100, 900);
        // 取整到 100 的倍数
        self.subtitle.subtitle_font_weight = (self.subtitle.subtitle_font_weight / 100) * 100;
        self.subtitle.subtitle_text_shadow_strength = self.subtitle.subtitle_text_shadow_strength.min(10);
        self.subtitle.subtitle_letter_spacing = self.subtitle.subtitle_letter_spacing.clamp(-5.0, 20.0);
        self.subtitle.subtitle_line_height = self.subtitle.subtitle_line_height.clamp(0.8, 3.0);
        // 对齐值归一
        self.subtitle.subtitle_text_align = match self.subtitle.subtitle_text_align.as_str() {
            "left" | "center" | "right" => self.subtitle.subtitle_text_align.clone(),
            _ => "center".to_string(),
        };
        // 元素级显示控制校验
        self.subtitle.subtitle_layout = match self.subtitle.subtitle_layout.as_str() {
            "vertical" | "horizontal" => self.subtitle.subtitle_layout.clone(),
            _ => "vertical".to_string(),
        };
        self.subtitle.subtitle_translation_opacity = self.subtitle.subtitle_translation_opacity.clamp(0.0, 1.0);
        self.subtitle.subtitle_translation_font_weight = self.subtitle.subtitle_translation_font_weight.clamp(100, 900);
        self.subtitle.subtitle_translation_font_weight = (self.subtitle.subtitle_translation_font_weight / 100) * 100;
        self.subtitle.subtitle_translation_font_size = self.subtitle.subtitle_translation_font_size.clamp(8, 96);
        self.subtitle.subtitle_speaker_font_size = self.subtitle.subtitle_speaker_font_size.clamp(8, 48);
        self.subtitle.subtitle_timestamp_font_size = self.subtitle.subtitle_timestamp_font_size.clamp(8, 48);
        self.subtitle.subtitle_timestamp_format = match self.subtitle.subtitle_timestamp_format.as_str() {
            "HH:MM:SS" | "MM:SS" | "none" => self.subtitle.subtitle_timestamp_format.clone(),
            _ => "HH:MM:SS".to_string(),
        };
        self.subtitle.subtitle_translation_engine = match self.subtitle.subtitle_translation_engine.as_str() {
            "none" | "llm" | "aliyun" | "deepl" => self.subtitle.subtitle_translation_engine.clone(),
            _ => "none".to_string(),
        };
        // 自定义元素系统校验
        self.subtitle.subtitle_preset = match self.subtitle.subtitle_preset.as_str() {
            "clean" | "bilingual" | "meeting" | "live" | "custom" => self.subtitle.subtitle_preset.clone(),
            _ => "clean".to_string(),
        };
        // 字幕识别音源类型校验
        self.subtitle.subtitle_audio_source = match self.subtitle.subtitle_audio_source.as_str() {
            "microphone" | "system" => self.subtitle.subtitle_audio_source.clone(),
            _ => "microphone".to_string(),
        };
        // 确保元素顺序包含所有固定元素
        for fixed in &["speaker", "original", "translation", "timestamp"] {
            if !self.subtitle.subtitle_element_order.iter().any(|e| e == fixed) {
                self.subtitle.subtitle_element_order.push((*fixed).to_string());
            }
        }
        // 校验自定义元素
        for (i, el) in self.subtitle.subtitle_custom_elements.iter_mut().enumerate() {
            el.element_type = match el.element_type.as_str() {
                "text" | "divider" | "spacer" => el.element_type.clone(),
                _ => "text".to_string(),
            };
            el.align = match el.align.as_str() {
                "left" | "center" | "right" => el.align.clone(),
                _ => "center".to_string(),
            };
            el.font_weight = (el.font_weight.clamp(100, 900) / 100) * 100;
            el.font_size = el.font_size.clamp(8, 96);
            el.opacity = el.opacity.clamp(0.0, 1.0);
            if el.id.is_empty() {
                el.id = format!("custom_{}", i);
            }
        }
        self.vad.vad_sensitivity = self.vad.vad_sensitivity.clamp(0.0, 1.0);
        // 音频采集偏好归一化（Profile 非法值一律回 standard）
        self.basic.audio_profile = match self.basic.audio_profile.as_str() {
            AUDIO_PROFILE_STANDARD | AUDIO_PROFILE_ARRAY_MIC | AUDIO_PROFILE_CUSTOM => {
                self.basic.audio_profile.clone()
            }
            _ => AUDIO_PROFILE_STANDARD.to_string(),
        };
        self.basic.audio_downmix = match self.basic.audio_downmix.as_str() {
            DOWNMIX_AVERAGE | DOWNMIX_STRONGEST | DOWNMIX_FIRST_CHANNEL => {
                self.basic.audio_downmix.clone()
            }
            _ => DOWNMIX_STRONGEST.to_string(),
        };
        self.basic.audio_sample_format = match self.basic.audio_sample_format.as_str() {
            SAMPLE_FMT_AUTO | SAMPLE_FMT_F32 | SAMPLE_FMT_I16 | SAMPLE_FMT_I32 => {
                self.basic.audio_sample_format.clone()
            }
            _ => SAMPLE_FMT_AUTO.to_string(),
        };
        self.basic.audio_sample_rate = match self.basic.audio_sample_rate.as_str() {
            SAMPLE_RATE_AUTO | SAMPLE_RATE_16K | SAMPLE_RATE_44K | SAMPLE_RATE_48K => {
                self.basic.audio_sample_rate.clone()
            }
            _ => SAMPLE_RATE_AUTO.to_string(),
        };
        self.basic.audio_channels = match self.basic.audio_channels.as_str() {
            CHANNELS_AUTO | CHANNELS_MONO | CHANNELS_STEREO => {
                self.basic.audio_channels.clone()
            }
            _ => CHANNELS_AUTO.to_string(),
        };
        // 多场景字幕窗口：迁移旧扁平配置并归一化
        self.normalize_subtitle_scenes();
    }

    /// 保证字幕场景列表有效：
    /// 1. 为空时从旧扁平字段迁移出 "default" 场景
    /// 2. 保证存在 "default" 场景
    /// 3. 逐个归一化场景字段
    /// 4. 限制场景数量上限
    fn normalize_subtitle_scenes(&mut self) {
        if self.subtitle.subtitle_scenes.is_empty() {
            let legacy = SubtitleSceneConfig::from_legacy(&self.subtitle);
            self.subtitle.subtitle_scenes.push(legacy);
        }
        if !self
            .subtitle
            .subtitle_scenes
            .iter()
            .any(|s| s.id == "default")
        {
            let mut default_scene = SubtitleSceneConfig::from_legacy(&self.subtitle);
            default_scene.id = "default".to_string();
            self.subtitle.subtitle_scenes.insert(0, default_scene);
        }
        // 场景 ID 去重（保留首个）
        let mut seen: Vec<String> = Vec::new();
        self.subtitle.subtitle_scenes.retain(|s| {
            if seen.contains(&s.id) {
                false
            } else {
                seen.push(s.id.clone());
                true
            }
        });
        for scene in self.subtitle.subtitle_scenes.iter_mut() {
            scene.normalize();
        }
        if self.subtitle.subtitle_scenes.len() > 8 {
            self.subtitle.subtitle_scenes.truncate(8);
        }
        // 兜底：默认场景不可被禁用（用户至少保留一个可用场景）
        let enabled_count = self
            .subtitle
            .subtitle_scenes
            .iter()
            .filter(|s| s.enabled)
            .count();
        if enabled_count == 0 {
            if let Some(scene) = self
                .subtitle
                .subtitle_scenes
                .iter_mut()
                .find(|s| s.id == "default")
            {
                scene.enabled = true;
            }
        }
    }

    fn normalize_streaming_post_process_mode(mode: &str) -> String {
        match mode {
            STREAMING_POST_AI | STREAMING_POST_LOCAL | STREAMING_POST_NONE => mode.to_string(),
            _ => STREAMING_POST_NONE.to_string(),
        }
    }

    fn normalize_dictation_mode(mode: &str) -> String {
        match mode {
            DICTATION_MODE_BATCH | DICTATION_MODE_STREAM => mode.to_string(),
            _ => DICTATION_MODE_BATCH.to_string(),
        }
    }
}

impl ConfigManager {
    pub fn new() -> Self {
        Self::new_with_base_dir(None)
    }

    pub fn new_with_base_dir(base_dir: Option<PathBuf>) -> Self {
        let config_dir = if let Some(dir) = base_dir {
            dir
        } else if let Some(proj_dirs) = directories::ProjectDirs::from("com", "guchang233", "voice2type")
        {
            proj_dirs.config_dir().to_path_buf()
        } else {
            PathBuf::from(".")
        };

        let _ = fs::create_dir_all(&config_dir);
        let path = config_dir.join("settings.json");

        let legacy_path = PathBuf::from("voice2type_config.json");
        let actual_path = if !path.exists() && legacy_path.exists() {
            legacy_path
        } else {
            path
        };

        let mut config = Self::load_config(&actual_path).unwrap_or_default();

        if !config.model_selection.batch_model.is_empty() {
            config.basic.model_name = config.model_selection.batch_model.clone();
        }

        if !actual_path.exists() {
            if let Ok(key) = std::env::var("SILICONFLOW_API_KEY") {
                config.model.siliconflow_api_key = key;
            }
        }

        let manager = Self {
            config: Arc::new(Mutex::new(config)),
            config_path: actual_path,
        };
        manager.migrate_legacy_whisper_dir();
        manager
    }

    pub fn models_dir(&self) -> PathBuf {
        // 优先使用用户自定义目录
        let custom = self.config.lock().unwrap().model.custom_models_dir.clone();
        if !custom.is_empty() && PathBuf::from(&custom).is_dir() {
            return PathBuf::from(custom);
        }
        // 默认目录：Windows 优先 D:\V2T\models（避免占用 C 盘）
        // 若 D 盘不存在，则回退到用户数据目录下的 models
        #[cfg(target_os = "windows")]
        {
            if std::path::Path::new("D:\\").exists() {
                return PathBuf::from("D:\\V2T\\models");
            }
        }
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Voice2Type")
            .join("models");
        data_dir
    }

    /// 获取当前生效的模型目录（只读，用于前端展示）
    pub fn current_models_dir(&self) -> String {
        self.models_dir().to_string_lossy().into_owned()
    }

    /// 设置自定义模型目录
    pub fn set_custom_models_dir(&self, dir: String) {
        self.config.lock().unwrap().model.custom_models_dir = dir;
    }

    /// 清除自定义模型目录（恢复默认）
    pub fn clear_custom_models_dir(&self) {
        self.config.lock().unwrap().model.custom_models_dir = String::new();
    }

    pub fn ensure_models_dir(&self) -> anyhow::Result<PathBuf> {
        let dir = self.models_dir();
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    pub fn whisper_models_dir(&self) -> PathBuf {
        // 模型 .bin 文件直接放在模型目录根目录（与前端展示和帮助教程一致）
        self.models_dir()
    }

    /// whisper.cpp 预编译二进制存放目录
    pub fn whisper_binary_dir(&self) -> PathBuf {
        self.models_dir().join("whisper-bin")
    }

    /// whisper.cpp 可执行文件完整路径（Windows: whisper-cli.exe）
    /// v1.9.2 起 main.exe 已弃用（仅输出警告后退出），改用 whisper-cli
    pub fn whisper_binary_path(&self) -> PathBuf {
        if cfg!(target_os = "windows") {
            self.whisper_binary_dir().join("whisper-cli.exe")
        } else {
            self.whisper_binary_dir().join("whisper-cli")
        }
    }



    fn migrate_legacy_whisper_dir(&self) {
        let legacy = self.config_dir().join("whisper");
        let mut cfg = self.config.lock().unwrap();
        if cfg.model.local_whisper_dir.is_empty() && legacy.is_dir() {
            cfg.model.local_whisper_dir = legacy.to_string_lossy().into_owned();
        }
    }

    fn load_config(path: &PathBuf) -> Option<AppConfig> {
        let content = fs::read_to_string(path).ok()?;

        if let Ok(mut loaded) = serde_json::from_str::<AppConfig>(&content) {
            loaded.initialize();
            return Some(loaded);
        }

        #[derive(Debug, Deserialize)]
        struct OldAppConfig {
            api_key: String,
            api_url: String,
            model_name: String,
        }

        let old = serde_json::from_str::<OldAppConfig>(&content).ok()?;
        let mut config = AppConfig::default();
        config.basic.model_name = old.model_name.clone();

        match old.model_name.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => config.model.siliconflow_api_key = old.api_key,
            MODEL_WHISPER => config.model.groq_api_key = old.api_key,
            _ => {
                config.basic.model_name = MODEL_CUSTOM.to_string();
                config.model.custom_model_name = old.model_name;
                config.model.custom_api_key = old.api_key;
                config.model.custom_api_url = old.api_url;
            }
        }

        Some(config)
    }

    pub fn get_config(&self) -> AppConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn set_config(&self, new_config: AppConfig) {
        let mut cfg = self.config.lock().unwrap();
        *cfg = new_config;
        cfg.basic.model_name = cfg.model_selection.batch_model.clone();
        cfg.normalize_subtitle_scenes();
    }

    fn effective_model_name(&self) -> String {
        let cfg = self.config.lock().unwrap();
        let batch = &cfg.model_selection.batch_model;
        if !batch.is_empty() {
            batch.clone()
        } else {
            cfg.basic.model_name.clone()
        }
    }

    pub fn is_local_whisper(&self) -> bool {
        self.effective_model_name() == MODEL_LOCAL_WHISPER
    }

    pub fn needs_api_key(&self) -> bool {
        self.effective_model_name() != MODEL_LOCAL_WHISPER
    }

    pub fn get_api_key(&self) -> String {
        let cfg = self.config.lock().unwrap();
        let model = if cfg.model_selection.batch_model.is_empty() {
            cfg.basic.model_name.clone()
        } else {
            cfg.model_selection.batch_model.clone()
        };
        match model.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => cfg.model.siliconflow_api_key.clone(),
            MODEL_WHISPER => cfg.model.groq_api_key.clone(),
            MODEL_LOCAL_WHISPER => String::new(),
            _ => cfg.model.custom_api_key.clone(),
        }
    }

    pub fn set_api_key(&self, key: String) {
        let mut cfg = self.config.lock().unwrap();
        let model = if cfg.model_selection.batch_model.is_empty() {
            cfg.basic.model_name.clone()
        } else {
            cfg.model_selection.batch_model.clone()
        };
        match model.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => cfg.model.siliconflow_api_key = key,
            MODEL_WHISPER => cfg.model.groq_api_key = key,
            _ => cfg.model.custom_api_key = key,
        }
    }

    pub fn get_siliconflow_api_key(&self) -> String {
        self.config.lock().unwrap().model.siliconflow_api_key.clone()
    }

    pub fn set_siliconflow_api_key(&self, key: String) {
        self.config.lock().unwrap().model.siliconflow_api_key = key;
    }

    pub fn get_groq_api_key(&self) -> String {
        self.config.lock().unwrap().model.groq_api_key.clone()
    }

    pub fn set_groq_api_key(&self, key: String) {
        self.config.lock().unwrap().model.groq_api_key = key;
    }

    pub fn get_doubao_api_key(&self) -> String {
        self.config.lock().unwrap().model.doubao_api_key.clone()
    }

    pub fn set_doubao_api_key(&self, key: String) {
        self.config.lock().unwrap().model.doubao_api_key = key;
    }

    pub fn streaming_hotkey(&self) -> u32 {
        self.config.lock().unwrap().streaming.hotkey
    }

    pub fn set_streaming_hotkey(&self, vk: u32) {
        self.config.lock().unwrap().streaming.hotkey = vk;
    }

    pub fn streaming_trigger_mode(&self) -> String {
        self.config.lock().unwrap().streaming.trigger_mode.clone()
    }

    pub fn set_streaming_trigger_mode(&self, mode: String) {
        self.config.lock().unwrap().streaming.trigger_mode = mode;
    }

    pub fn streaming_resource_id(&self) -> String {
        self.config.lock().unwrap().streaming.resource_id.clone()
    }

    pub fn set_streaming_resource_id(&self, id: String) {
        self.config.lock().unwrap().streaming.resource_id = id;
    }

    pub fn streaming_model_name(&self) -> String {
        self.config.lock().unwrap().streaming.model_name.clone()
    }

    pub fn streaming_output_language(&self) -> String {
        self.config.lock().unwrap().streaming.output_language.clone()
    }

    pub fn set_streaming_output_language(&self, lang: String) {
        self.config.lock().unwrap().streaming.output_language = lang;
    }

    pub fn streaming_allow_emoji(&self) -> bool {
        self.config.lock().unwrap().streaming.allow_emoji
    }

    pub fn set_streaming_allow_emoji(&self, allow: bool) {
        self.config.lock().unwrap().streaming.allow_emoji = allow;
    }

    pub fn streaming_allow_punctuation(&self) -> bool {
        self.config.lock().unwrap().streaming.allow_punctuation
    }

    pub fn set_streaming_allow_punctuation(&self, allow: bool) {
        self.config.lock().unwrap().streaming.allow_punctuation = allow;
    }

    pub fn streaming_enable_indicator(&self) -> bool {
        self.config.lock().unwrap().streaming.enable_indicator
    }

    pub fn set_streaming_enable_indicator(&self, enable: bool) {
        self.config.lock().unwrap().streaming.enable_indicator = enable;
    }

    pub fn streaming_post_process_mode(&self) -> String {
        self.config.lock().unwrap().streaming.post_process_mode.clone()
    }

    pub fn set_streaming_post_process_mode(&self, mode: String) {
        self.config.lock().unwrap().streaming.post_process_mode = mode;
    }

    pub fn get_api_url(&self) -> String {
        let cfg = self.config.lock().unwrap();
        let model = if cfg.model_selection.batch_model.is_empty() {
            cfg.basic.model_name.clone()
        } else {
            cfg.model_selection.batch_model.clone()
        };
        match model.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => SILICONFLOW_TRANSCRIPTIONS_URL.to_string(),
            MODEL_WHISPER => GROQ_TRANSCRIPTIONS_URL.to_string(),
            _ => cfg.model.custom_api_url.clone(),
        }
    }

    pub fn set_api_url(&self, url: String) {
        let mut cfg = self.config.lock().unwrap();
        if cfg.basic.model_name == MODEL_CUSTOM {
            cfg.model.custom_api_url = url;
        }
    }

    pub fn get_model_id(&self) -> String {
        self.effective_model_name()
    }

    pub fn get_model_name(&self) -> String {
        let cfg = self.config.lock().unwrap();
        let model = if cfg.model_selection.batch_model.is_empty() {
            cfg.basic.model_name.clone()
        } else {
            cfg.model_selection.batch_model.clone()
        };
        if model == MODEL_CUSTOM {
            cfg.model.custom_model_name.clone()
        } else if model == MODEL_LOCAL_WHISPER {
            format!(
                "本地 Whisper ({})",
                if cfg.model.local_whisper_model.is_empty() {
                    "ggml-base.bin"
                } else {
                    &cfg.model.local_whisper_model
                }
            )
        } else {
            model
        }
    }

    pub fn set_model_name(&self, model: String) {
        let mut cfg = self.config.lock().unwrap();
        match model.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE | MODEL_WHISPER | MODEL_LOCAL_WHISPER => {
                cfg.basic.model_name = model
            }
            _ => {
                cfg.basic.model_name = MODEL_CUSTOM.to_string();
                cfg.model.custom_model_name = model;
            }
        }
    }

    pub fn local_whisper_model(&self) -> String {
        self.config.lock().unwrap().model.local_whisper_model.clone()
    }

    pub fn set_local_whisper_model(&self, name: String) {
        self.config.lock().unwrap().model.local_whisper_model = name;
    }

    pub fn local_whisper_threads(&self) -> u32 {
        self.config.lock().unwrap().model.local_whisper_threads
    }

    pub fn set_local_whisper_threads(&self, threads: u32) {
        self.config.lock().unwrap().model.local_whisper_threads = threads;
    }

    pub fn local_whisper_greedy(&self) -> bool {
        self.config.lock().unwrap().model.local_whisper_greedy
    }

    pub fn set_local_whisper_greedy(&self, greedy: bool) {
        self.config.lock().unwrap().model.local_whisper_greedy = greedy;
    }

    pub fn local_whisper_no_fallback(&self) -> bool {
        self.config.lock().unwrap().model.local_whisper_no_fallback
    }

    pub fn set_local_whisper_no_fallback(&self, no_fallback: bool) {
        self.config.lock().unwrap().model.local_whisper_no_fallback = no_fallback;
    }

    pub fn local_whisper_detected_language(&self) -> String {
        self.config.lock()
            .unwrap()
            .model
            .local_whisper_detected_language
            .clone()
    }

    pub fn set_local_whisper_detected_language(&self, lang: String) {
        self.config.lock().unwrap().model.local_whisper_detected_language = lang;
    }

    pub fn local_whisper_dir(&self) -> String {
        self.config.lock().unwrap().model.local_whisper_dir.clone()
    }

    pub fn set_local_whisper_dir(&self, path: String) {
        self.config.lock().unwrap().model.local_whisper_dir = path;
    }

    pub fn has_local_whisper_dir(&self) -> bool {
        !self.local_whisper_dir().is_empty()
    }

    pub fn allow_emoji(&self) -> bool {
        self.config.lock().unwrap().features.allow_emoji
    }

    pub fn set_allow_emoji(&self, allow: bool) {
        self.config.lock().unwrap().features.allow_emoji = allow;
    }

    pub fn allow_punctuation(&self) -> bool {
        self.config.lock().unwrap().features.allow_punctuation
    }

    pub fn set_allow_punctuation(&self, allow: bool) {
        self.config.lock().unwrap().features.allow_punctuation = allow;
    }

    pub fn show_log(&self) -> bool {
        self.config.lock().unwrap().basic.show_log
    }

    pub fn set_show_log(&self, show: bool) {
        self.config.lock().unwrap().basic.show_log = show;
    }

    pub fn input_device(&self) -> String {
        self.config.lock().unwrap().basic.input_device.clone()
    }

    pub fn set_input_device(&self, name: String) {
        self.config.lock().unwrap().basic.input_device = name;
    }

    // ===== 音频采集偏好：返回「有效」值（考虑 Profile 覆盖）=====

    /// 解析得到「生效」的音频采集参数（Profile 会覆盖单项，除 custom 外）。
    /// 返回 (downmix, sample_fmt, sample_rate, channels)。
    pub fn effective_audio_prefs(&self) -> (String, String, String, String) {
        let cfg = self.config.lock().unwrap();
        let b = &cfg.basic;
        let profile = b.audio_profile.as_str();
        let (mut dm, mut sf, mut sr, mut ch) = (
            b.audio_downmix.clone(),
            b.audio_sample_format.clone(),
            b.audio_sample_rate.clone(),
            b.audio_channels.clone(),
        );
        match profile {
            AUDIO_PROFILE_STANDARD => {
                dm = DOWNMIX_STRONGEST.to_string();
                sf = SAMPLE_FMT_AUTO.to_string();
                sr = SAMPLE_RATE_AUTO.to_string();
                ch = CHANNELS_AUTO.to_string();
            }
            AUDIO_PROFILE_ARRAY_MIC => {
                dm = DOWNMIX_STRONGEST.to_string();
                sf = SAMPLE_FMT_F32.to_string();
                sr = SAMPLE_RATE_48K.to_string();
                ch = CHANNELS_AUTO.to_string();
            }
            _ => {
                // custom：保持单项值不变
            }
        }
        (dm, sf, sr, ch)
    }

    pub fn audio_profile(&self) -> String {
        self.config.lock().unwrap().basic.audio_profile.clone()
    }
    pub fn set_audio_profile(&self, v: String) {
        self.config.lock().unwrap().basic.audio_profile = v;
    }
    pub fn audio_downmix(&self) -> String {
        self.config.lock().unwrap().basic.audio_downmix.clone()
    }
    pub fn set_audio_downmix(&self, v: String) {
        self.config.lock().unwrap().basic.audio_downmix = v;
    }
    pub fn audio_sample_format(&self) -> String {
        self.config.lock().unwrap().basic.audio_sample_format.clone()
    }
    pub fn set_audio_sample_format(&self, v: String) {
        self.config.lock().unwrap().basic.audio_sample_format = v;
    }
    pub fn audio_sample_rate(&self) -> String {
        self.config.lock().unwrap().basic.audio_sample_rate.clone()
    }
    pub fn set_audio_sample_rate(&self, v: String) {
        self.config.lock().unwrap().basic.audio_sample_rate = v;
    }
    pub fn audio_channels(&self) -> String {
        self.config.lock().unwrap().basic.audio_channels.clone()
    }
    pub fn set_audio_channels(&self, v: String) {
        self.config.lock().unwrap().basic.audio_channels = v;
    }

    pub fn subtitle_input_device(&self) -> String {
        self.config.lock().unwrap().subtitle.subtitle_input_device.clone()
    }

    pub fn set_subtitle_input_device(&self, name: String) {
        self.config.lock().unwrap().subtitle.subtitle_input_device = name;
    }

    pub fn subtitle_audio_source(&self) -> String {
        self.config.lock().unwrap().subtitle.subtitle_audio_source.clone()
    }

    // ===== 多场景字幕窗口 =====

    pub fn get_subtitle_scenes(&self) -> Vec<SubtitleSceneConfig> {
        self.config.lock().unwrap().subtitle.subtitle_scenes.clone()
    }

    /// 新增字幕场景（默认复制 default 场景样式），返回新场景 ID
    pub fn add_subtitle_scene(&self) -> Result<String, String> {
        let mut cfg = self.config.lock().unwrap();
        let base = cfg
            .subtitle
            .subtitle_scenes
            .iter()
            .find(|s| s.id == "default")
            .cloned()
            .unwrap_or_else(|| SubtitleSceneConfig::from_legacy(&cfg.subtitle));
        let id = format!("sc_{}", uuid::Uuid::new_v4().simple());
        let name = format!("字幕窗口 {}", cfg.subtitle.subtitle_scenes.len() + 1);
        let mut scene = base;
        scene.id = id.clone();
        scene.name = name;
        scene.enabled = true;
        cfg.subtitle.subtitle_scenes.push(scene);
        cfg.normalize_subtitle_scenes();
        Ok(id)
    }

    /// 复制指定场景为新场景，返回新场景 ID
    pub fn duplicate_subtitle_scene(&self, scene_id: &str) -> Result<String, String> {
        let mut cfg = self.config.lock().unwrap();
        let base = cfg
            .subtitle
            .subtitle_scenes
            .iter()
            .find(|s| s.id == scene_id)
            .cloned()
            .ok_or_else(|| "要复制的场景不存在".to_string())?;
        let id = format!("sc_{}", uuid::Uuid::new_v4().simple());
        let mut scene = base;
        scene.id = id.clone();
        scene.name = format!("{} 副本", scene.name);
        scene.enabled = true;
        cfg.subtitle.subtitle_scenes.push(scene);
        cfg.normalize_subtitle_scenes();
        Ok(id)
    }

    /// 删除场景（default 不可删除）
    pub fn remove_subtitle_scene(&self, scene_id: &str) -> Result<(), String> {
        if scene_id == "default" {
            return Err("默认场景不可删除".to_string());
        }
        let mut cfg = self.config.lock().unwrap();
        let before = cfg.subtitle.subtitle_scenes.len();
        cfg.subtitle
            .subtitle_scenes
            .retain(|s| s.id != scene_id);
        if cfg.subtitle.subtitle_scenes.len() == before {
            return Err("场景不存在".to_string());
        }
        cfg.normalize_subtitle_scenes();
        Ok(())
    }

    /// 窗口拖动/缩放后持久化几何信息（按场景）
    pub fn update_subtitle_scene_window(
        &self,
        scene_id: &str,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) {
        let mut cfg = self.config.lock().unwrap();
        if let Some(scene) = cfg
            .subtitle
            .subtitle_scenes
            .iter_mut()
            .find(|s| s.id == scene_id)
        {
            scene.window.x = x;
            scene.window.y = y;
            scene.window.width = width.max(200).min(3840);
            scene.window.height = height.max(40).min(2160);
        }
    }

    /// 窗口关闭时停用场景
    pub fn set_subtitle_scene_enabled(&self, scene_id: &str, enabled: bool) {
        let mut cfg = self.config.lock().unwrap();
        if let Some(scene) = cfg
            .subtitle
            .subtitle_scenes
            .iter_mut()
            .find(|s| s.id == scene_id)
        {
            scene.enabled = enabled;
        }
    }

    pub fn set_subtitle_scene_window_flag(
        &self,
        scene_id: &str,
        flag: &str,
        value: bool,
    ) {
        let mut cfg = self.config.lock().unwrap();
        if let Some(scene) = cfg
            .subtitle
            .subtitle_scenes
            .iter_mut()
            .find(|s| s.id == scene_id)
        {
            match flag {
                "always_on_top" => scene.window.always_on_top = value,
                "click_through" => scene.window.click_through = value,
                "obs_mode" => scene.window.obs_mode = value,
                _ => {}
            }
        }
    }

    // ===== 同声传译 LLM 配置 =====

    pub fn subtitle_translation_llm_api_url(&self) -> String {
        self.config
            .lock()
            .unwrap()
            .subtitle
            .subtitle_translation_llm_api_url
            .clone()
    }

    pub fn subtitle_translation_llm_api_key(&self) -> String {
        self.config
            .lock()
            .unwrap()
            .subtitle
            .subtitle_translation_llm_api_key
            .clone()
    }

    pub fn subtitle_translation_llm_model(&self) -> String {
        self.config
            .lock()
            .unwrap()
            .subtitle
            .subtitle_translation_llm_model
            .clone()
    }

    pub fn output_mode(&self) -> String {
        self.config.lock().unwrap().basic.output_mode.clone()
    }

    pub fn set_output_mode(&self, mode: String) {
        self.config.lock().unwrap().basic.output_mode = mode;
    }

    pub fn autostart_enabled(&self) -> bool {
        self.config.lock().unwrap().basic.autostart
    }

    pub fn set_autostart_enabled(&self, enabled: bool) {
        self.config.lock().unwrap().basic.autostart = enabled;
    }

    pub fn hotkey(&self) -> u32 {
        self.config.lock().unwrap().basic.hotkey
    }

    pub fn set_hotkey(&self, vk: u32) {
        self.config.lock().unwrap().basic.hotkey = vk;
    }

    pub fn enable_indicator(&self) -> bool {
        self.config.lock().unwrap().features.enable_indicator
    }

    pub fn set_enable_indicator(&self, enable: bool) {
        self.config.lock().unwrap().features.enable_indicator = enable;
    }

    /// 是否启用增强后处理链（PostProcessorChain + TextFormatter）。默认 false。
    pub fn enable_post_processor(&self) -> bool {
        self.config.lock().unwrap().features.enable_post_processor
    }

    pub fn set_enable_post_processor(&self, enable: bool) {
        self.config.lock().unwrap().features.enable_post_processor = enable;
    }

    /// 是否启用 LLM 智能后处理校对。默认 false。
    pub fn llm_post_enable(&self) -> bool {
        self.config.lock().unwrap().llm_post.enable
    }

    pub fn set_llm_post_enable(&self, enable: bool) {
        self.config.lock().unwrap().llm_post.enable = enable;
    }

    pub fn llm_post_api_url(&self) -> String {
        self.config.lock().unwrap().llm_post.api_url.clone()
    }

    pub fn set_llm_post_api_url(&self, url: String) {
        self.config.lock().unwrap().llm_post.api_url = url;
    }

    pub fn llm_post_api_key(&self) -> String {
        self.config.lock().unwrap().llm_post.api_key.clone()
    }

    pub fn set_llm_post_api_key(&self, key: String) {
        self.config.lock().unwrap().llm_post.api_key = key;
    }

    pub fn llm_post_model(&self) -> String {
        self.config.lock().unwrap().llm_post.model.clone()
    }

    pub fn set_llm_post_model(&self, model: String) {
        self.config.lock().unwrap().llm_post.model = model;
    }

    pub fn llm_post_system_prompt(&self) -> String {
        self.config.lock().unwrap().llm_post.system_prompt.clone()
    }

    pub fn set_llm_post_system_prompt(&self, prompt: String) {
        self.config.lock().unwrap().llm_post.system_prompt = prompt;
    }

    pub fn last_check_time(&self) -> u64 {
        self.config.lock().unwrap().update.last_check_time
    }

    pub fn set_last_check_time(&self, time: u64) {
        self.config.lock().unwrap().update.last_check_time = time;
    }

    pub fn ignored_version(&self) -> String {
        self.config.lock().unwrap().update.ignored_version.clone()
    }

    pub fn set_ignored_version(&self, version: String) {
        self.config.lock().unwrap().update.ignored_version = version;
    }

    pub fn output_language(&self) -> String {
        self.config.lock().unwrap().basic.output_language.clone()
    }

    pub fn set_output_language(&self, lang: String) {
        self.config.lock().unwrap().basic.output_language = lang;
    }

    pub fn trigger_mode(&self) -> String {
        self.config.lock().unwrap().advanced.trigger_mode.clone()
    }

    pub fn set_trigger_mode(&self, mode: String) {
        self.config.lock().unwrap().advanced.trigger_mode = mode;
    }

    pub fn dictation_mode(&self) -> String {
        self.config.lock().unwrap().basic.dictation_mode.clone()
    }

    pub fn set_dictation_mode(&self, mode: String) {
        self.config.lock().unwrap().basic.dictation_mode = mode;
    }

    pub fn is_stream_mode(&self) -> bool {
        self.dictation_mode() == DICTATION_MODE_STREAM
    }

    pub fn get_speech_service(&self) -> String {
        let model = self.effective_model_name();
        match model.as_str() {
            MODEL_TELEAI | MODEL_SENSEVOICE => "siliconflow".to_string(),
            MODEL_WHISPER => "groq".to_string(),
            MODEL_LOCAL_WHISPER => "local".to_string(),
            _ => "custom".to_string(),
        }
    }

    pub fn indicator_fade_duration(&self) -> u64 {
        self.config.lock().unwrap().indicator.fade_duration
    }

    pub fn set_indicator_fade_duration(&self, duration: u64) {
        self.config.lock().unwrap().indicator.fade_duration = duration;
    }

    pub fn indicator_error_duration(&self) -> u64 {
        self.config.lock().unwrap().indicator.error_duration
    }

    pub fn set_indicator_error_duration(&self, duration: u64) {
        self.config.lock().unwrap().indicator.error_duration = duration;
    }

    pub fn indicator_success_duration(&self) -> u64 {
        self.config.lock().unwrap().indicator.success_duration
    }

    pub fn set_indicator_success_duration(&self, duration: u64) {
        self.config.lock().unwrap().indicator.success_duration = duration;
    }

    pub fn subtitle_hotkey(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_hotkey
    }

    pub fn set_subtitle_hotkey(&self, vk: u32) {
        self.config.lock().unwrap().subtitle.subtitle_hotkey = vk;
    }

    pub fn subtitle_font_size(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_font_size
    }

    pub fn set_subtitle_font_size(&self, size: u32) {
        self.config.lock().unwrap().subtitle.subtitle_font_size = size;
    }

    pub fn subtitle_font_color(&self) -> String {
        self.config.lock().unwrap().subtitle.subtitle_font_color.clone()
    }

    pub fn set_subtitle_font_color(&self, color: String) {
        self.config.lock().unwrap().subtitle.subtitle_font_color = color;
    }

    pub fn subtitle_bg_opacity(&self) -> f32 {
        self.config.lock().unwrap().subtitle.subtitle_bg_opacity
    }

    pub fn set_subtitle_bg_opacity(&self, opacity: f32) {
        self.config.lock().unwrap().subtitle.subtitle_bg_opacity = opacity.clamp(0.0, 1.0);
    }

    pub fn subtitle_blur(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_blur
    }

    pub fn set_subtitle_blur(&self, blur: u32) {
        self.config.lock().unwrap().subtitle.subtitle_blur = blur;
    }

    pub fn subtitle_max_lines(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_max_lines
    }

    pub fn set_subtitle_max_lines(&self, lines: u32) {
        self.config.lock().unwrap().subtitle.subtitle_max_lines = lines;
    }

    pub fn subtitle_window_x(&self) -> i32 {
        self.config.lock().unwrap().subtitle.subtitle_window_x
    }

    pub fn set_subtitle_window_x(&self, x: i32) {
        self.config.lock().unwrap().subtitle.subtitle_window_x = x;
    }

    pub fn subtitle_window_y(&self) -> i32 {
        self.config.lock().unwrap().subtitle.subtitle_window_y
    }

    pub fn set_subtitle_window_y(&self, y: i32) {
        self.config.lock().unwrap().subtitle.subtitle_window_y = y;
    }

    pub fn subtitle_window_width(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_window_width
    }

    pub fn set_subtitle_window_width(&self, width: u32) {
        self.config.lock().unwrap().subtitle.subtitle_window_width = width;
    }

    pub fn subtitle_window_height(&self) -> u32 {
        self.config.lock().unwrap().subtitle.subtitle_window_height
    }

    pub fn set_subtitle_window_height(&self, height: u32) {
        self.config.lock().unwrap().subtitle.subtitle_window_height = height;
    }

    pub fn vad_sensitivity(&self) -> f32 {
        self.config.lock().unwrap().vad.vad_sensitivity
    }

    pub fn set_vad_sensitivity(&self, sensitivity: f32) {
        self.config.lock().unwrap().vad.vad_sensitivity = sensitivity.clamp(0.0, 1.0);
    }

    pub fn vad_silence_duration_ms(&self) -> u32 {
        self.config.lock().unwrap().vad.vad_silence_duration_ms
    }

    pub fn set_vad_silence_duration_ms(&self, duration: u32) {
        self.config.lock().unwrap().vad.vad_silence_duration_ms = duration;
    }

    pub fn batch_model(&self) -> String {
        self.config.lock().unwrap().model_selection.batch_model.clone()
    }

    pub fn set_batch_model(&self, model: String) {
        self.config.lock().unwrap().model_selection.batch_model = model;
    }

    pub fn stream_model(&self) -> String {
        self.config.lock().unwrap().model_selection.stream_model.clone()
    }

    pub fn set_stream_model(&self, model: String) {
        self.config.lock().unwrap().model_selection.stream_model = model;
    }

    pub fn subtitle_model(&self) -> String {
        self.config.lock().unwrap().model_selection.subtitle_model.clone()
    }

    pub fn set_subtitle_model(&self, model: String) {
        self.config.lock().unwrap().model_selection.subtitle_model = model;
    }

    pub fn theme(&self) -> String {
        self.config.lock().unwrap().theme.clone()
    }

    pub fn set_theme(&self, theme: String) {
        self.config.lock().unwrap().theme = theme;
    }

    pub fn reset_ai_config(&self) {
        let mut cfg = self.config.lock().unwrap();
        cfg.basic.model_name = MODEL_SENSEVOICE.to_string();
        cfg.model = ModelConfig::default();
        cfg.advanced.trigger_mode = AdvancedConfig::default().trigger_mode;
        cfg.indicator = IndicatorConfig::default();
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config = self.config.lock().unwrap();
        let json = serde_json::to_string_pretty(&*config)?;
        fs::write(&self.config_path, json)?;
        Ok(())
    }

    pub fn save_or_notify(&self) -> bool {
        match self.save() {
            Ok(()) => true,
            Err(e) => {
                crate::notify::queue_tray_message("配置保存失败", &e.to_string());
                false
            }
        }
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_path.clone()
    }

    pub fn config_dir(&self) -> PathBuf {
        self.config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn get_custom_model_name(&self) -> String {
        self.config.lock().unwrap().model.custom_model_name.clone()
    }

    pub fn set_custom_model_name(&self, name: String) {
        let mut cfg = self.config.lock().unwrap();
        cfg.model.custom_model_name = name;
    }

    pub fn tts_config(&self) -> TtsConfig {
        self.config.lock().unwrap().tts.clone()
    }

    pub fn set_tts_config(&self, tts: TtsConfig) {
        let mut cfg = self.config.lock().unwrap();
        cfg.tts = tts;
    }

    pub fn log_dir(&self) -> PathBuf {
        self.config_path
            .parent()
            .map(|path| path.join("logs"))
            .unwrap_or_else(|| PathBuf::from("logs"))
    }

    pub fn log_file_path(&self) -> PathBuf {
        self.log_dir().join("app.log")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证旧版扁平配置 → 多场景结构的迁移与持久化
    #[test]
    fn subtitle_scene_migration_and_persist() {
        let dir = std::env::temp_dir().join(format!("v2t_mig_test_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();

        // 模拟旧版配置：无 subtitle_scenes，只有扁平字段
        let legacy = serde_json::json!({
            "subtitle": {
                "subtitle_font_size": 40,
                "subtitle_show_translation": true,
                "subtitle_translation_engine": "none",
                "subtitle_translation_target_lang": "en",
                "subtitle_window_x": 100,
                "subtitle_window_y": 200,
                "subtitle_custom_elements": [{"id":"c1","element_type":"text","content":"hello"}]
            }
        });
        std::fs::write(dir.join("settings.json"), legacy.to_string()).unwrap();

        let mgr = ConfigManager::new_with_base_dir(Some(dir.clone()));
        let cfg = mgr.get_config();
        assert_eq!(cfg.subtitle.subtitle_scenes.len(), 1, "应迁移出一个 default 场景");
        let scene = &cfg.subtitle.subtitle_scenes[0];
        assert_eq!(scene.id, "default");
        assert_eq!(scene.style.font_size, 40);
        assert_eq!(scene.style.show_translation, true);
        assert_eq!(scene.window.x, 100);
        assert_eq!(scene.window.y, 200);
        assert_eq!(scene.style.custom_elements.len(), 1);

        // 保存后重新加载仍然有效
        mgr.save().unwrap();
        let mgr2 = ConfigManager::new_with_base_dir(Some(dir.clone()));
        assert_eq!(mgr2.get_config().subtitle.subtitle_scenes.len(), 1);

        // 场景增删
        let new_id = mgr2.add_subtitle_scene().unwrap();
        assert_eq!(mgr2.get_config().subtitle.subtitle_scenes.len(), 2);
        let dup_id = mgr2.duplicate_subtitle_scene(&new_id).unwrap();
        assert_eq!(mgr2.get_config().subtitle.subtitle_scenes.len(), 3);
        mgr2.remove_subtitle_scene(&dup_id).unwrap();
        assert_eq!(mgr2.get_config().subtitle.subtitle_scenes.len(), 2);
        assert!(mgr2.remove_subtitle_scene("default").is_err(), "默认场景不可删除");

        // set_config 也会触发场景归一化（前端保存路径）
        let mut cfg3 = mgr2.get_config();
        cfg3.subtitle.subtitle_scenes.clear();
        mgr2.set_config(cfg3);
        assert_eq!(mgr2.get_config().subtitle.subtitle_scenes.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }
}

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

// ==================== 实时字幕（v3 新模型：窗口 + 主题 + 元素） ====================
//
// 架构（中央引擎 + 信号拉取）：SubtitleEngine 统一处理 音频采集 → 豆包流式 ASR →
// 句段/说话人追踪 → 逐窗口翻译 → 权威快照；字幕窗口订阅轻量信号后按需拉取渲染。
//
// 配置模型：SubtitleSettings = 全局音源/热键/LLM 端点 + windows[]。
// 每个窗口 = 窗口控制 + 翻译配置 + 主题（分组样式）+ 统一元素列表
// （固定与自定义元素混合，数组顺序即显示顺序）。

/// 主窗口 ID（对应 tauri.conf.json 中的静态 "subtitle" 窗口）
pub const PRIMARY_WINDOW_ID: &str = "primary";

/// 固定元素 kind 集合（不可删除，仅可开关/排序）
pub const FIXED_ELEMENT_KINDS: [&str; 5] =
    ["speaker", "original", "translation", "secondary", "timestamp"];

fn default_audio_source() -> String { "microphone".to_string() }
fn default_layout() -> String { "vertical".to_string() }
fn default_subtitle_hotkey() -> u32 { 0x76 }
fn default_translation_target_lang() -> String { "英文".to_string() }
fn default_preset() -> String { "custom".to_string() }
fn default_timestamp_format() -> String { "HH:MM:SS".to_string() }
fn default_white() -> String { "#ffffff".to_string() }
fn default_custom_font_size() -> u32 { 18 }
fn default_custom_opacity() -> f32 { 0.9 }

/// 同声传译 LLM 端点（全局共享；api_key 留空时回退「LLM 智能校对」配置）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleLlmConfig {
    pub api_url: String,
    pub api_key: String,
    pub model: String,
}

impl Default for SubtitleLlmConfig {
    fn default() -> Self {
        Self {
            api_url: String::new(),
            api_key: String::new(),
            model: String::new(),
        }
    }
}

/// 文本子样式（译文等元素）。
/// `size == 0` 表示「自动」：副原文取原文字号 × 0.8（下限 14px）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTextStyle {
    pub size: u32,
    pub weight: u32,
    pub color: String,
    pub opacity: f32,
    pub prefix: String,
}

impl Default for SubtitleTextStyle {
    fn default() -> Self {
        Self {
            size: 24,
            weight: 400,
            color: "#ffffff".to_string(),
            opacity: 0.85,
            prefix: String::new(),
        }
    }
}

/// 说话人样式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSpeakerStyle {
    pub color: String,
    pub size: u32,
    pub prefix: String,
}

impl Default for SubtitleSpeakerStyle {
    fn default() -> Self {
        Self {
            color: "#818cf8".to_string(),
            size: 16,
            prefix: String::new(),
        }
    }
}

/// 时间戳样式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTimestampStyle {
    pub color: String,
    pub size: u32,
    pub format: String,
}

impl Default for SubtitleTimestampStyle {
    fn default() -> Self {
        Self {
            color: "#a1a1aa".to_string(),
            size: 14,
            format: default_timestamp_format(),
        }
    }
}

/// 副原文（双源同传的麦克风原声）样式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSecondaryStyle {
    pub color: String,
    pub size: u32,
    pub opacity: f32,
}

impl Default for SubtitleSecondaryStyle {
    fn default() -> Self {
        Self {
            color: "#7dd3fc".to_string(),
            size: 0,
            opacity: 0.9,
        }
    }
}

/// 窗口主题（分组样式，替代旧版 40+ 扁平字段）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTheme {
    /// 预设模板：clean | bilingual | meeting | live | custom
    pub preset: String,
    pub font_family: String,
    pub font_size: u32,
    pub font_weight: u32,
    pub italic: bool,
    /// 原文文字颜色
    pub font_color: String,
    pub text_align: String,
    pub letter_spacing: f32,
    pub line_height: f32,
    pub text_shadow_color: String,
    pub text_shadow_strength: u32,
    pub interim_color: String,
    pub interim_opacity: f32,
    pub bg_color: String,
    pub bg_opacity: f32,
    pub blur: u32,
    pub padding_x: u32,
    pub padding_y: u32,
    pub max_lines: u32,
    /// 布局方向：vertical | horizontal
    pub layout: String,
    /// 窗口内水平锚点：left | center | right
    pub anchor_x: String,
    /// 窗口内垂直锚点：top | center | bottom
    pub anchor_y: String,
    /// 卡片最大宽度（窗口宽度的百分比 30-100）
    pub max_width_pct: u32,
    pub translation: SubtitleTextStyle,
    pub speaker: SubtitleSpeakerStyle,
    pub timestamp: SubtitleTimestampStyle,
    pub secondary: SubtitleSecondaryStyle,
}

impl Default for SubtitleTheme {
    fn default() -> Self {
        Self {
            preset: default_preset(),
            font_family: "SimHei".to_string(),
            font_size: 32,
            font_weight: 400,
            italic: false,
            font_color: default_white(),
            text_align: "center".to_string(),
            letter_spacing: 0.0,
            line_height: 1.4,
            text_shadow_color: "#000000".to_string(),
            text_shadow_strength: 4,
            interim_color: default_white(),
            interim_opacity: 0.7,
            bg_color: "#000000".to_string(),
            bg_opacity: 0.6,
            blur: 20,
            padding_x: 24,
            padding_y: 12,
            max_lines: 3,
            layout: default_layout(),
            anchor_x: "center".to_string(),
            anchor_y: "bottom".to_string(),
            max_width_pct: 100,
            translation: SubtitleTextStyle::default(),
            speaker: SubtitleSpeakerStyle::default(),
            timestamp: SubtitleTimestampStyle::default(),
            secondary: SubtitleSecondaryStyle::default(),
        }
    }
}

/// 统一元素（固定与自定义混合；数组顺序 = 显示顺序）。
///
/// kind ∈ speaker|original|translation|secondary|timestamp（固定）
///       | text|divider|spacer（自定义）。
/// 固定元素样式来自 theme；自定义元素样式来自自身字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleElement {
    pub kind: String,
    /// 固定元素 id 与 kind 同名；自定义元素 id 唯一（如 "c_1"）
    pub id: String,
    pub enabled: bool,
    /// 设置面板展示名
    pub label: String,
    /// 自定义 text 内容，支持占位符 {time} {date} {datetime} {text} {translation} {speaker}
    pub content: String,
    pub prefix: String,
    pub color: String,
    /// spacer 的「字号」字段复用为高度 px
    pub font_size: u32,
    pub font_weight: u32,
    pub opacity: f32,
    pub align: String,
}

impl Default for SubtitleElement {
    fn default() -> Self {
        Self {
            kind: "text".to_string(),
            id: String::new(),
            enabled: true,
            label: "文本".to_string(),
            content: String::new(),
            prefix: String::new(),
            color: default_white(),
            font_size: default_custom_font_size(),
            font_weight: 400,
            opacity: default_custom_opacity(),
            align: "center".to_string(),
        }
    }
}

impl SubtitleElement {
    /// 构造固定元素（id 与 kind 相同）
    pub fn fixed(kind: &str, label: &str, enabled: bool) -> Self {
        Self {
            kind: kind.to_string(),
            id: kind.to_string(),
            enabled,
            label: label.to_string(),
            ..Self::default()
        }
    }
}

/// 单窗口翻译配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleTranslationConfig {
    /// none | llm
    pub engine: String,
    pub target_lang: String,
    /// 是否实时翻译中间结果（同声预览）
    pub interim: bool,
}

impl Default for SubtitleTranslationConfig {
    fn default() -> Self {
        Self {
            engine: "none".to_string(),
            target_lang: default_translation_target_lang(),
            interim: true,
        }
    }
}

/// 一个字幕窗口 = 一个独立的置顶字幕卡片
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleWindow {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// -1 = 未设置（使用系统默认位置）
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub always_on_top: bool,
    pub click_through: bool,
    /// OBS 捕捉兼容模式（默认开启）：纯黑背景，采集画面最干净
    pub obs_mode: bool,
    pub auto_fit: bool,
    pub translation: SubtitleTranslationConfig,
    pub theme: SubtitleTheme,
    pub elements: Vec<SubtitleElement>,
}

impl Default for SubtitleWindow {
    fn default() -> Self {
        Self {
            id: PRIMARY_WINDOW_ID.to_string(),
            name: "默认字幕".to_string(),
            enabled: true,
            x: -1,
            y: -1,
            width: 1200,
            height: 120,
            always_on_top: true,
            click_through: false,
            obs_mode: true,
            auto_fit: true,
            translation: SubtitleTranslationConfig::default(),
            theme: SubtitleTheme::default(),
            elements: vec![
                SubtitleElement::fixed("speaker", "说话人", false),
                SubtitleElement::fixed("original", "原文", true),
                SubtitleElement::fixed("translation", "译文", true),
                SubtitleElement::fixed("secondary", "副原文（麦克风）", false),
                SubtitleElement::fixed("timestamp", "时间戳", false),
            ],
        }
    }
}

impl SubtitleWindow {
    /// 校验并归一化（非法值回退默认）
    pub fn normalize(&mut self) {
        if self.id.trim().is_empty() {
            self.id = PRIMARY_WINDOW_ID.to_string();
        }
        if self.name.trim().is_empty() {
            self.name = "字幕窗口".to_string();
        }
        if self.width == 0 {
            self.width = 1200;
        }
        if self.height == 0 {
            self.height = 120;
        }
        self.width = self.width.clamp(200, 3840);
        self.height = self.height.clamp(40, 2160);

        let t = &mut self.theme;
        t.preset = match t.preset.as_str() {
            "clean" | "bilingual" | "meeting" | "live" | "custom" => t.preset.clone(),
            _ => default_preset(),
        };
        t.font_size = t.font_size.clamp(12, 96);
        t.font_weight = (t.font_weight.clamp(100, 900) / 100) * 100;
        t.text_align = match t.text_align.as_str() {
            "left" | "center" | "right" => t.text_align.clone(),
            _ => "center".to_string(),
        };
        t.letter_spacing = t.letter_spacing.clamp(-5.0, 20.0);
        t.line_height = t.line_height.clamp(0.8, 3.0);
        t.text_shadow_strength = t.text_shadow_strength.min(10);
        t.interim_opacity = t.interim_opacity.clamp(0.0, 1.0);
        t.bg_opacity = t.bg_opacity.clamp(0.0, 1.0);
        t.blur = t.blur.min(40);
        t.max_lines = t.max_lines.clamp(1, 6);
        t.layout = match t.layout.as_str() {
            "vertical" | "horizontal" => t.layout.clone(),
            _ => default_layout(),
        };
        t.anchor_x = match t.anchor_x.as_str() {
            "left" | "center" | "right" => t.anchor_x.clone(),
            _ => "center".to_string(),
        };
        t.anchor_y = match t.anchor_y.as_str() {
            "top" | "center" | "bottom" => t.anchor_y.clone(),
            _ => "bottom".to_string(),
        };
        if t.max_width_pct == 0 {
            t.max_width_pct = 100;
        }
        t.max_width_pct = t.max_width_pct.clamp(30, 100);
        t.translation.size = t.translation.size.clamp(8, 96);
        t.translation.weight = (t.translation.weight.clamp(100, 900) / 100) * 100;
        t.translation.opacity = t.translation.opacity.clamp(0.0, 1.0);
        t.speaker.size = t.speaker.size.clamp(8, 48);
        t.timestamp.size = t.timestamp.size.clamp(8, 48);
        t.timestamp.format = match t.timestamp.format.as_str() {
            "HH:MM:SS" | "MM:SS" | "none" => t.timestamp.format.clone(),
            _ => default_timestamp_format(),
        };
        t.secondary.opacity = t.secondary.opacity.clamp(0.0, 1.0);
        if t.secondary.size > 0 {
            t.secondary.size = t.secondary.size.clamp(8, 96);
        }

        let tr = &mut self.translation;
        tr.engine = match tr.engine.as_str() {
            "none" | "llm" => tr.engine.clone(),
            _ => "none".to_string(),
        };
        if tr.target_lang.trim().is_empty() {
            tr.target_lang = default_translation_target_lang();
        }

        // 固定元素：缺失则补到末尾；重复则去重（保留首个）
        let mut seen_fixed: Vec<String> = Vec::new();
        self.elements.retain(|el| {
            if FIXED_ELEMENT_KINDS.contains(&el.kind.as_str()) {
                if seen_fixed.contains(&el.kind) {
                    false
                } else {
                    seen_fixed.push(el.kind.clone());
                    true
                }
            } else {
                true
            }
        });
        for fixed in FIXED_ELEMENT_KINDS {
            if !self.elements.iter().any(|el| el.kind == fixed) {
                let (label, enabled) = match fixed {
                    "speaker" => ("说话人", false),
                    "original" => ("原文", true),
                    "translation" => ("译文", true),
                    "secondary" => ("副原文（麦克风）", false),
                    _ => ("时间戳", false),
                };
                self.elements.push(SubtitleElement::fixed(fixed, label, enabled));
            }
        }
        // 元素字段归一
        for (i, el) in self.elements.iter_mut().enumerate() {
            if !FIXED_ELEMENT_KINDS.contains(&el.kind.as_str()) {
                el.kind = match el.kind.as_str() {
                    "text" | "divider" | "spacer" => el.kind.clone(),
                    _ => "text".to_string(),
                };
            }
            el.id = if FIXED_ELEMENT_KINDS.contains(&el.kind.as_str()) {
                el.kind.clone()
            } else if el.id.is_empty() {
                format!("c_{}", i + 1)
            } else {
                el.id.clone()
            };
            if el.label.is_empty() {
                el.label = el.kind.clone();
            }
            el.font_weight = (el.font_weight.clamp(100, 900) / 100) * 100;
            el.font_size = el.font_size.clamp(0, 96);
            el.opacity = el.opacity.clamp(0.0, 1.0);
            el.align = match el.align.as_str() {
                "left" | "center" | "right" => el.align.clone(),
                _ => "center".to_string(),
            };
        }
    }
}

/// 实时字幕设置（v3）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleSettings {
    /// 字幕开关全局热键（Windows VK 码，F7 = 0x76）
    pub hotkey: u32,
    /// microphone | system | dual
    pub audio_source: String,
    pub input_device: String,
    /// 同声传译 LLM 端点（全局共享）
    pub translation_llm: SubtitleLlmConfig,
    pub windows: Vec<SubtitleWindow>,
}

impl Default for SubtitleSettings {
    fn default() -> Self {
        Self {
            hotkey: default_subtitle_hotkey(),
            audio_source: default_audio_source(),
            input_device: String::new(),
            translation_llm: SubtitleLlmConfig::default(),
            windows: vec![SubtitleWindow::default()],
        }
    }
}

impl SubtitleSettings {
    /// 校验并归一化：保证 primary 存在、固定元素齐全、至少一个启用窗口。
    pub fn normalize(&mut self) {
        self.audio_source = match self.audio_source.as_str() {
            "microphone" | "system" | "dual" => self.audio_source.clone(),
            _ => default_audio_source(),
        };

        if self.windows.is_empty() {
            self.windows.push(SubtitleWindow::default());
        }
        if !self.windows.iter().any(|w| w.id == PRIMARY_WINDOW_ID) {
            let mut primary = SubtitleWindow::default();
            primary.id = PRIMARY_WINDOW_ID.to_string();
            self.windows.insert(0, primary);
        }
        // ID 去重（保留首个）
        let mut seen: Vec<String> = Vec::new();
        self.windows.retain(|w| {
            if seen.contains(&w.id) {
                false
            } else {
                seen.push(w.id.clone());
                true
            }
        });
        if self.windows.len() > 8 {
            self.windows.truncate(8);
        }
        for w in self.windows.iter_mut() {
            w.normalize();
        }
        // 至少保留一个启用窗口
        if !self.windows.iter().any(|w| w.enabled) {
            if let Some(w) = self.windows.iter_mut().find(|w| w.id == PRIMARY_WINDOW_ID) {
                w.enabled = true;
            }
        }
    }

    /// 已启用的窗口（会话只向这些窗口供流）
    pub fn enabled_windows(&self) -> Vec<SubtitleWindow> {
        self.windows
            .iter()
            .filter(|w| w.enabled)
            .cloned()
            .collect()
    }

    pub fn window(&self, id: &str) -> Option<&SubtitleWindow> {
        self.windows.iter().find(|w| w.id == id)
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
    pub subtitle: SubtitleSettings,
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
            subtitle: SubtitleSettings::default(),
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
        self.subtitle.normalize();
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

                // 先做 JSON 级迁移（v2 平铺/场景模型 → v3 窗口+主题+元素模型），再反序列化
        let mut value: serde_json::Value = serde_json::from_str(&content).ok()?;
        crate::subtitle::migration::migrate_subtitle_json(&mut value);
        if let Ok(mut loaded) = serde_json::from_value::<AppConfig>(value) {
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
        cfg.subtitle.normalize();
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

    // ===== 实时字幕（v3） =====

    pub fn subtitle_input_device(&self) -> String {
        self.config.lock().unwrap().subtitle.input_device.clone()
    }

    pub fn set_subtitle_input_device(&self, name: String) {
        self.config.lock().unwrap().subtitle.input_device = name;
    }

    pub fn subtitle_audio_source(&self) -> String {
        self.config.lock().unwrap().subtitle.audio_source.clone()
    }

    pub fn get_subtitle_windows(&self) -> Vec<SubtitleWindow> {
        self.config.lock().unwrap().subtitle.windows.clone()
    }

    /// 新增字幕窗口（复制当前窗口），返回新窗口 ID
    pub fn add_subtitle_window(&self) -> Result<String, String> {
        let mut cfg = self.config.lock().unwrap();
        let base = cfg.subtitle.windows.first().cloned().unwrap_or_default();
        let id = format!("w_{}", uuid::Uuid::new_v4().simple());
        let mut win = base;
        win.id = id.clone();
        win.name = format!("字幕窗口 {}", cfg.subtitle.windows.len() + 1);
        win.enabled = true;
        win.x = -1;
        win.y = -1;
        cfg.subtitle.windows.push(win);
        cfg.subtitle.normalize();
        Ok(id)
    }

    /// 复制指定窗口，返回新窗口 ID
    pub fn duplicate_subtitle_window(&self, window_id: &str) -> Result<String, String> {
        let mut cfg = self.config.lock().unwrap();
        let base = cfg
            .subtitle
            .windows
            .iter()
            .find(|w| w.id == window_id)
            .cloned()
            .ok_or_else(|| "要复制的窗口不存在".to_string())?;
        let id = format!("w_{}", uuid::Uuid::new_v4().simple());
        let mut win = base;
        win.id = id.clone();
        win.name = format!("{} 副本", win.name);
        win.enabled = true;
        win.x = -1;
        win.y = -1;
        cfg.subtitle.windows.push(win);
        cfg.subtitle.normalize();
        Ok(id)
    }

    /// 删除窗口（primary 不可删除）
    pub fn remove_subtitle_window(&self, window_id: &str) -> Result<(), String> {
        if window_id == PRIMARY_WINDOW_ID {
            return Err("默认字幕窗口不可删除".to_string());
        }
        let mut cfg = self.config.lock().unwrap();
        let before = cfg.subtitle.windows.len();
        cfg.subtitle
            .windows
            .retain(|w| w.id != window_id);
        if cfg.subtitle.windows.len() == before {
            return Err("窗口不存在".to_string());
        }
        cfg.subtitle.normalize();
        Ok(())
    }

    /// 窗口拖动/缩放后持久化几何信息
    pub fn update_subtitle_window_rect(
        &self,
        window_id: &str,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) {
        let mut cfg = self.config.lock().unwrap();
        if let Some(w) = cfg.subtitle.windows.iter_mut().find(|w| w.id == window_id) {
            w.x = x;
            w.y = y;
            w.width = width.max(200).min(3840);
            w.height = height.max(40).min(2160);
        }
    }

    /// 窗口关闭时停用窗口
    pub fn set_subtitle_window_enabled(&self, window_id: &str, enabled: bool) {
        let mut cfg = self.config.lock().unwrap();
        if let Some(w) = cfg.subtitle.windows.iter_mut().find(|w| w.id == window_id) {
            w.enabled = enabled;
        }
    }

    /// 窗口控制开关（置顶/穿透/OBS 兼容/自适应）
    pub fn set_subtitle_window_flag(&self, window_id: &str, flag: &str, value: bool) {
        let mut cfg = self.config.lock().unwrap();
        if let Some(w) = cfg.subtitle.windows.iter_mut().find(|w| w.id == window_id) {
            match flag {
                "always_on_top" => w.always_on_top = value,
                "click_through" => w.click_through = value,
                "obs_mode" => w.obs_mode = value,
                "auto_fit" => w.auto_fit = value,
                _ => {}
            }
        }
    }

    // ===== 同声传译 LLM 配置（全局共享） =====

    pub fn subtitle_translation_llm_api_url(&self) -> String {
        self.config.lock().unwrap().subtitle.translation_llm.api_url.clone()
    }

    pub fn subtitle_translation_llm_api_key(&self) -> String {
        self.config.lock().unwrap().subtitle.translation_llm.api_key.clone()
    }

    pub fn subtitle_translation_llm_model(&self) -> String {
        self.config.lock().unwrap().subtitle.translation_llm.model.clone()
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
        self.config.lock().unwrap().subtitle.hotkey
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

    #[test]
    fn subtitle_v3_defaults() {
        let s = SubtitleSettings::default();
        assert_eq!(s.audio_source, "microphone");
        assert_eq!(s.windows.len(), 1);
        let w = &s.windows[0];
        assert_eq!(w.id, PRIMARY_WINDOW_ID);
        assert_eq!(w.theme.font_size, 32);
        assert_eq!(w.theme.bg_color, "#000000");
        assert_eq!(w.theme.max_lines, 3);
        let kinds: Vec<&str> = w.elements.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["speaker", "original", "translation", "secondary", "timestamp"]
        );
        assert!(!w.elements[0].enabled);
        assert!(w.elements[1].enabled);
    }

    #[test]
    fn subtitle_v3_normalize_keeps_fixed_elements() {
        let mut w = SubtitleWindow::default();
        w.elements.retain(|e| e.kind == "original"); // 删到只剩原文
        w.normalize();
        let kinds: Vec<&str> = w.elements.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds.len(), 5, "固定元素被补齐");
        assert!(kinds.contains(&"speaker"));
        assert!(kinds.contains(&"original"));
    }

    #[test]
    fn subtitle_v3_legacy_migration_roundtrip() {
        let dir = std::env::temp_dir().join(format!("v2t_mig_test_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        // 模拟 v2 配置：平铺字段 + 一个场景
        let legacy = serde_json::json!({
            "subtitle": {
                "subtitle_hotkey": 119,
                "subtitle_audio_source": "dual",
                "subtitle_font_size": 40,
                "subtitle_show_translation": true,
                "subtitle_translation_engine": "none",
                "subtitle_translation_target_lang": "en",
                "subtitle_window_x": 100,
                "subtitle_window_y": 200,
                "subtitle_custom_elements": [{"id":"c1","element_type":"text","content":"hello"}],
                "subtitle_scenes": [{
                    "id": "default",
                    "name": "默认字幕",
                    "enabled": true,
                    "window": {"x": 10, "y": 20, "width": 900, "height": 150, "always_on_top": true},
                    "style": {"font_size": 40, "show_translation": true},
                    "translation": {"engine": "none", "target_lang": "en"}
                }]
            }
        });
        std::fs::write(dir.join("settings.json"), legacy.to_string()).unwrap();

        let mgr = ConfigManager::new_with_base_dir(Some(dir.clone()));
        let cfg = mgr.get_config();
        assert_eq!(cfg.subtitle.audio_source, "dual");
        assert_eq!(cfg.subtitle.windows.len(), 1);
        let w = &cfg.subtitle.windows[0];
        assert_eq!(w.id, PRIMARY_WINDOW_ID);
        assert_eq!(w.theme.font_size, 40);
        assert_eq!(w.x, 10);
        assert_eq!(w.y, 20);

        // 保存后重新加载仍然有效
        mgr.save().unwrap();
        let mgr2 = ConfigManager::new_with_base_dir(Some(dir.clone()));
        assert_eq!(mgr2.get_config().subtitle.windows.len(), 1);

        // 窗口增删
        let new_id = mgr2.add_subtitle_window().unwrap();
        assert_eq!(mgr2.get_config().subtitle.windows.len(), 2);
        let dup_id = mgr2.duplicate_subtitle_window(&new_id).unwrap();
        assert_eq!(mgr2.get_config().subtitle.windows.len(), 3);
        mgr2.remove_subtitle_window(&dup_id).unwrap();
        assert_eq!(mgr2.get_config().subtitle.windows.len(), 2);
        assert!(mgr2.remove_subtitle_window(PRIMARY_WINDOW_ID).is_err(), "默认窗口不可删除");

        // set_config 也会触发归一化
        let mut cfg3 = mgr2.get_config();
        cfg3.subtitle.windows.clear();
        mgr2.set_config(cfg3);
        assert_eq!(mgr2.get_config().subtitle.windows.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }
}

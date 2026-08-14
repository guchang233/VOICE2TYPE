//! 同声传译：可插拔翻译引擎 + 增量句段翻译流水线（主动推送上屏）
//!
//! 设计要点：
//! 1. `TranslationEngine` trait 定义引擎接口，`resolve_engine` 注册表按配置名解析，
//!    未来接入阿里云/DeepL 等只需实现 trait 并注册；
//! 2. 只翻译「已确定」（definite）文本中新增的完整句段，避免重复翻译；
//! 3. 每个句段分配递增 seq，译文乱序返回时按 seq 顺序落地，保证字幕不跳变；
//! 4. 「临时」（indefinite）+ 未成句尾部的译文作为「当前行」实时预览，
//!    防抖 1 秒内合并请求；句段定稿后当前行译文进入历史；
//! 5. 译文落地即通过 dirty 通道通知会话重发帧（不再等待下一帧 ASR）；
//! 6. 多场景共享翻译缓存（语言+原文 → 译文），避免相同内容重复调用 LLM。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::{mpsc, Mutex};

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;

/// 当前行（临时）译文最小请求间隔
const CURRENT_MIN_INTERVAL_MS: u64 = 1000;
/// 单次翻译请求超时
const TRANSLATE_TIMEOUT_SECS: u64 = 15;
/// 未翻译的 definite 尾部超过该长度时强制作为句段翻译（防止长句等待标点过久）
const FORCE_SEGMENT_CHARS: usize = 40;
/// 历史行最大保留数
const HISTORY_LIMIT: usize = 8;

// ==================== 翻译引擎抽象 ====================

/// 翻译引擎接口：每个实现负责把一段文本翻译成目标语言。
/// 引擎需 `Send + Sync`（会话在异步任务间共享）。
#[async_trait]
pub trait TranslationEngine: Send + Sync {
    /// 引擎标识名（与配置中的 subtitle_translation_engine 对应）
    fn name(&self) -> &'static str;

    /// 翻译一段文本
    async fn translate(
        &self,
        config: &ConfigManager,
        target_lang: &str,
        text: &str,
    ) -> Result<String>;
}

/// 按配置名解析翻译引擎（返回 None 表示未注册/关闭）
pub fn resolve_engine(name: &str) -> Option<Arc<dyn TranslationEngine>> {
    match name {
        "llm" => Some(Arc::new(LlmTranslationEngine)),
        // 预留： "aliyun" / "deepl" 实现 trait 后在此注册
        _ => None,
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

/// LLM 翻译引擎：OpenAI 兼容 chat/completions 接口
///
/// 接口配置优先级：字幕翻译专用 LLM 配置 → 「LLM 智能校对」配置 → 内置默认（SiliconFlow）。
pub struct LlmTranslationEngine;

#[async_trait]
impl TranslationEngine for LlmTranslationEngine {
    fn name(&self) -> &'static str {
        "llm"
    }

    async fn translate(
        &self,
        config: &ConfigManager,
        target_lang: &str,
        text: &str,
    ) -> Result<String> {
        let input = text.trim();
        if input.is_empty() {
            return Ok(String::new());
        }

        let mut url = config.subtitle_translation_llm_api_url();
        let mut key = config.subtitle_translation_llm_api_key();
        let mut model = config.subtitle_translation_llm_model();
        if key.is_empty() {
            // 回退：复用「LLM 智能校对」的接口配置
            url = config.llm_post_api_url();
            key = config.llm_post_api_key();
            model = config.llm_post_model();
        }
        if url.trim().is_empty() {
            url = "https://api.siliconflow.cn/v1/chat/completions".to_string();
        }
        if model.trim().is_empty() {
            model = "Qwen/Qwen2.5-7B-Instruct".to_string();
        }
        if key.is_empty() {
            anyhow::bail!("同声传译 LLM API Key 未配置（可在字幕设置或「设置-LLM 智能校对」中填写）");
        }

        let system = "你是专业的同声传译员，负责把实时语音识别的文字翻译成目标语言。\
规则：1. 只输出译文本身，不要任何解释、引号或前后缀；\
2. 保持原意完整，译文符合口语习惯；\
3. 数字、专有名词原样保留；\
4. 如果原文已经是目标语言，直接原样输出；\
5. 输入的可能是未完成的口语片段，请直接翻译其字面意思，不要补充或猜测。";

        let user = format!(
            "请把下面的语音识别原文翻译成{}。只输出译文本身，不要任何解释、引号或前后缀；若原文已经是{}则原样输出。\n\n原文：\n{}",
            target_lang, target_lang, input
        );

        let body = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ],
            "temperature": 0.3,
            "max_tokens": 1024,
            "stream": false,
        });

        let resp = tokio::time::timeout(
            Duration::from_secs(TRANSLATE_TIMEOUT_SECS),
            HTTP_CLIENT
                .post(&url)
                .header("Authorization", format!("Bearer {}", key))
                .json(&body)
                .send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("翻译请求超时"))??;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            let brief: String = body_text.chars().take(200).collect();
            anyhow::bail!("翻译接口错误 {}: {}", status, brief);
        }

        let chat: ChatResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("翻译响应解析失败: {}", e))?;
        let content = chat
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        Ok(clean_translation(&content))
    }
}

/// 清理引擎输出：去首尾空白/引号/「」/换行
fn clean_translation(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.len() >= 2 {
        let first = s.chars().next().unwrap();
        let last = s.chars().last().unwrap();
        let paired = (first == '"' && last == '"')
            || (first == '“' && last == '”')
            || (first == '「' && last == '」');
        if paired {
            s = s.chars().skip(1).take(s.chars().count() - 2).collect();
            s = s.trim().to_string();
        }
    }
    s
}

// ==================== 翻译流水线 ====================

struct TranslationState {
    /// 已定稿句段译文（按顺序，最多 HISTORY_LIMIT 条）
    history: VecDeque<String>,
    /// definite 文本已消费的字符数
    translated_chars: usize,
    /// 乱序保护：seq → 译文
    pending: HashMap<u64, String>,
    /// 下一个分配的 seq（从 1 开始，0 为 applied_seq 初始值）
    next_seq: u64,
    /// 已按序落地的最大 seq
    applied_seq: u64,
    /// 当前行译文目标文本（未成句尾部 + 临时文本）
    current_target: String,
    /// 当前行译文（同声预览）
    current: String,
    /// 当前行代际：只有最新一代的结果才会被采纳
    current_gen: u64,
    last_current_at: Option<Instant>,
}

impl TranslationState {
    fn new() -> Self {
        Self {
            history: VecDeque::new(),
            translated_chars: 0,
            pending: HashMap::new(),
            next_seq: 1,
            applied_seq: 0,
            current_target: String::new(),
            current: String::new(),
            current_gen: 0,
            last_current_at: None,
        }
    }
}

/// 每场景一个翻译流水线：句段历史 + 当前行译文 + 主动推送
pub struct TranslationPipeline {
    engine: Option<Arc<dyn TranslationEngine>>,
    target_lang: String,
    interim_enabled: bool,
    state: Arc<Mutex<TranslationState>>,
    config: Arc<ConfigManager>,
    /// 跨场景共享翻译缓存：key = "lang\u{1}text"
    cache: Arc<Mutex<HashMap<String, String>>>,
    /// 译文落地/变化时通知会话重发帧（主动推送，保证翻译实时上屏）
    dirty_tx: mpsc::UnboundedSender<()>,
}

impl TranslationPipeline {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<ConfigManager>,
        engine: Option<Arc<dyn TranslationEngine>>,
        target_lang: String,
        interim_enabled: bool,
        cache: Arc<Mutex<HashMap<String, String>>>,
        dirty_tx: mpsc::UnboundedSender<()>,
    ) -> Self {
        Self {
            engine,
            target_lang,
            interim_enabled,
            state: Arc::new(Mutex::new(TranslationState::new())),
            config,
            cache,
            dirty_tx,
        }
    }

    /// 引擎是否有效
    pub fn active(&self) -> bool {
        self.engine.is_some()
    }

    /// 处理一帧 ASR 结果：更新句段翻译与当前行译文
    pub async fn on_frame(&self, definite: &str, indefinite: &str) {
        if !self.active() {
            return;
        }

        let d = definite.trim();
        let i = indefinite.trim();

        let mut segment_spawns: Vec<(u64, String)> = Vec::new();
        let mut current_spawn: Option<(u64, String)> = None;
        {
            let mut st = self.state.lock().await;
            let d_chars: Vec<char> = d.chars().collect();

            // ASR 文本异常回退（definite 变短）：重置累计状态
            if st.translated_chars > d_chars.len() {
                st.history.clear();
                st.translated_chars = 0;
                st.pending.clear();
                st.applied_seq = st.next_seq.saturating_sub(1);
                st.current.clear();
                st.current_target.clear();
                st.current_gen += 1;
            }

            let new_part: String = d_chars[st.translated_chars..].iter().collect();
            let mut complete = Vec::new();
            let mut tail = String::new();
            if !new_part.is_empty() {
                let (comp, t) = split_complete_sentences(&new_part);
                complete = comp;
                tail = t;
                st.translated_chars += new_part.chars().count() - tail.chars().count();
            }

            for sentence in complete {
                let seq = st.next_seq;
                st.next_seq += 1;
                segment_spawns.push((seq, sentence));
            }

            // 长尾强制切段（避免长时间等标点）
            if tail.chars().count() >= FORCE_SEGMENT_CHARS {
                let seq = st.next_seq;
                st.next_seq += 1;
                segment_spawns.push((seq, tail.clone()));
                st.translated_chars = d_chars.len();
                tail.clear();
            }

            // ===== 当前行译文（未成句尾部 + 临时文本，防抖） =====
            if self.interim_enabled {
                let new_target = if tail.is_empty() {
                    i.to_string()
                } else if i.is_empty() {
                    tail.clone()
                } else {
                    format!("{} {}", tail, i)
                };
                let target_changed = new_target != st.current_target;
                let due = st.last_current_at.map_or(true, |t| {
                    t.elapsed() >= Duration::from_millis(CURRENT_MIN_INTERVAL_MS)
                });
                if !new_target.is_empty() && (target_changed || due) {
                    st.last_current_at = Some(Instant::now());
                    st.current_target = new_target.clone();
                    st.current_gen += 1;
                    current_spawn = Some((st.current_gen, new_target));
                } else if new_target.is_empty() && !st.current_target.is_empty() {
                    // 说话停顿：清空当前行目标（已显示的译文保留到句段定稿）
                    st.current_target.clear();
                }
            }
        }

        // 锁外派发任务
        for (seq, sentence) in segment_spawns {
            self.spawn_definite(seq, sentence);
        }
        if let Some((gen, text)) = current_spawn {
            self.spawn_current(gen, text);
        }
    }

    /// 快照当前可见译文：(历史行, 当前行)
    pub async fn snapshot(&self) -> (Vec<String>, String) {
        if !self.active() {
            return (Vec::new(), String::new());
        }
        let st = self.state.lock().await;
        (st.history.iter().cloned().collect(), st.current.clone())
    }

    fn spawn_definite(&self, seq: u64, sentence: String) {
        let Some(engine) = self.engine.clone() else { return };
        let state = self.state.clone();
        let config = self.config.clone();
        let cache = self.cache.clone();
        let lang = self.target_lang.clone();
        let dirty = self.dirty_tx.clone();
        tokio::spawn(async move {
            let translated = translate_with_cache(&config, &cache, engine.as_ref(), &lang, &sentence)
                .await
                .unwrap_or_else(|e| {
                    log::warn!("[同声传译] 句段翻译失败: {}", e);
                    String::new()
                });
            {
                let mut st = state.lock().await;
                // 过期任务（状态已重置）直接丢弃
                if seq <= st.applied_seq {
                    return;
                }
                st.pending.insert(seq, translated);
                // 按序落地
                loop {
                    let next = st.applied_seq + 1;
                    match st.pending.remove(&next) {
                        Some(t) => {
                            if !t.is_empty() {
                                if st.history.len() >= HISTORY_LIMIT {
                                    st.history.pop_front();
                                }
                                st.history.push_back(t);
                            }
                            st.applied_seq = next;
                        }
                        None => break,
                    }
                }
            }
            let _ = dirty.send(());
        });
    }

    fn spawn_current(&self, gen: u64, text: String) {
        let Some(engine) = self.engine.clone() else { return };
        let state = self.state.clone();
        let config = self.config.clone();
        let cache = self.cache.clone();
        let lang = self.target_lang.clone();
        let dirty = self.dirty_tx.clone();
        tokio::spawn(async move {
            let translated = translate_with_cache(&config, &cache, engine.as_ref(), &lang, &text)
                .await
                .unwrap_or_else(|e| {
                    log::debug!("[同声传译] 当前行翻译失败: {}", e);
                    String::new()
                });
            {
                let mut st = state.lock().await;
                if st.current_gen == gen && !translated.is_empty() {
                    st.current = translated;
                }
            }
            let _ = dirty.send(());
        });
    }
}

/// 切分完整句段：返回 (完整句段列表, 未完结尾部)
pub fn split_complete_sentences(text: &str) -> (Vec<String>, String) {
    let mut sentences = Vec::new();
    let mut last_end = 0usize;
    let mut tail_start = 0usize;
    let chars: Vec<char> = text.chars().collect();
    for (idx, &c) in chars.iter().enumerate() {
        if matches!(c, '。' | '！' | '？' | '!' | '?' | ';' | '；' | '\n') {
            let end = idx + 1;
            let sentence: String = chars[last_end..end].iter().collect();
            let trimmed = sentence.trim().to_string();
            if is_meaningful(&trimmed) {
                sentences.push(trimmed);
            }
            last_end = end;
            tail_start = end;
        }
    }
    let tail: String = chars[tail_start..].iter().collect();
    (sentences, tail)
}

/// 判断文本是否含有实质内容（字母/数字/CJK 等）
fn is_meaningful(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_alphanumeric() || (c as u32) > 0x2E80)
}

/// 带缓存翻译：先查缓存，未命中则调用引擎并写入缓存
async fn translate_with_cache(
    config: &Arc<ConfigManager>,
    cache: &Arc<Mutex<HashMap<String, String>>>,
    engine: &dyn TranslationEngine,
    target_lang: &str,
    text: &str,
) -> Result<String> {
    let input = text.trim();
    if input.is_empty() {
        return Ok(String::new());
    }
    let cache_key = format!("{}\u{1}{}", target_lang, input);
    {
        let c = cache.lock().await;
        if let Some(hit) = c.get(&cache_key) {
            return Ok(hit.clone());
        }
    }

    let translated = engine.translate(config, target_lang, input).await?;

    let mut c = cache.lock().await;
    if c.len() > 2048 {
        c.clear();
    }
    c.insert(cache_key, translated.clone());
    Ok(translated)
}

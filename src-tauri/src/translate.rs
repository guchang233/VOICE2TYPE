//! 同声传译：增量句段翻译流水线
//!
//! 设计要点：
//! 1. 只翻译「已确定」（definite）文本中新增的完整句段，避免重复翻译；
//! 2. 每个句段分配递增 seq，译文乱序返回时按 seq 顺序落地，保证字幕不跳变；
//! 3. 「临时」（indefinite）文本做防抖翻译，作为同声预览，定稿后自动被正式译文替换；
//! 4. 多场景共享翻译缓存（语言+原文 → 译文），避免相同内容重复调用 LLM。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;

/// 临时译文防抖间隔：距上一次请求至少间隔这么久才发起新的预览翻译
const PROVISIONAL_MIN_INTERVAL_MS: u64 = 1200;
/// 单次翻译请求超时
const TRANSLATE_TIMEOUT_SECS: u64 = 15;
/// 未翻译的 definite 尾部超过该长度时强制作为句段翻译（防止长句等待标点过久）
const FORCE_SEGMENT_CHARS: usize = 60;

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

struct TranslationState {
    /// 已定稿译文（按句段顺序拼接）
    final_translation: String,
    /// definite 文本已消费的字符数
    translated_chars: usize,
    /// 乱序保护：seq → 译文
    pending: HashMap<u64, String>,
    /// 下一个分配的 seq
    next_seq: u64,
    /// 已按序落地的最大 seq
    applied_seq: u64,
    /// 临时译文（同声预览）
    provisional: String,
    /// 临时译文代际：只有最新一代的结果才会被采纳
    provisional_gen: u64,
    last_provisional_at: Option<Instant>,
}

impl TranslationState {
    fn new() -> Self {
        Self {
            final_translation: String::new(),
            translated_chars: 0,
            pending: HashMap::new(),
            // seq 从 1 开始分配，0 作为 applied_seq 的初始值，
            // 使守卫 `seq <= applied_seq` 不会误丢弃第一句
            next_seq: 1,
            applied_seq: 0,
            provisional: String::new(),
            provisional_gen: 0,
            last_provisional_at: None,
        }
    }
}

pub struct TranslationPipeline {
    engine: String,
    target_lang: String,
    interim_enabled: bool,
    state: Arc<Mutex<TranslationState>>,
    config: Arc<ConfigManager>,
    /// 跨场景共享翻译缓存：key = "lang\u{1}text"
    cache: Arc<Mutex<HashMap<String, String>>>,
}

impl TranslationPipeline {
    pub fn new(
        config: Arc<ConfigManager>,
        engine: String,
        target_lang: String,
        interim_enabled: bool,
        cache: Arc<Mutex<HashMap<String, String>>>,
    ) -> Self {
        Self {
            engine,
            target_lang,
            interim_enabled,
            state: Arc::new(Mutex::new(TranslationState::new())),
            config,
            cache,
        }
    }

    /// 引擎是否有效（"llm" 才启用水线）
    pub fn active(&self) -> bool {
        self.engine == "llm"
    }

    /// 处理一帧 ASR 结果，返回 (定稿译文, 临时译文)
    pub async fn on_frame(&self, definite: &str, indefinite: &str) -> (String, String) {
        if !self.active() {
            return (String::new(), String::new());
        }

        let d = definite.trim();
        let i = indefinite.trim();

        // ===== 1. 提取新增完整句段并派发翻译任务 =====
        let mut segment_spawns: Vec<(u64, String)> = Vec::new();
        let mut provisional_spawn: Option<(u64, String)> = None;
        {
            let mut st = self.state.lock().await;
            let d_chars: Vec<char> = d.chars().collect();

            // ASR 文本异常回退（definite 变短）：重置累计状态
            if st.translated_chars > d_chars.len() {
                st.final_translation.clear();
                st.translated_chars = 0;
                st.pending.clear();
                st.applied_seq = st.next_seq.saturating_sub(1);
                st.provisional.clear();
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

            // ===== 2. 临时译文（防抖 + 代际控制） =====
            if self.interim_enabled && !i.is_empty() {
                let due = st
                    .last_provisional_at
                    .map_or(true, |t| t.elapsed() >= Duration::from_millis(PROVISIONAL_MIN_INTERVAL_MS));
                if due {
                    st.last_provisional_at = Some(Instant::now());
                    st.provisional_gen += 1;
                    let gen = st.provisional_gen;
                    let text = if tail.is_empty() {
                        i.to_string()
                    } else {
                        format!("{} {}", tail, i)
                    };
                    provisional_spawn = Some((gen, text));
                }
            }
        }

        // 在锁外派发任务
        for (seq, sentence) in segment_spawns {
            self.spawn_definite(seq, sentence);
        }
        if let Some((gen, text)) = provisional_spawn {
            self.spawn_provisional(gen, text);
        }

        // ===== 3. 返回当前可见译文 =====
        let st = self.state.lock().await;
        (st.final_translation.clone(), st.provisional.clone())
    }

    fn spawn_definite(&self, seq: u64, sentence: String) {
        let state = self.state.clone();
        let config = self.config.clone();
        let cache = self.cache.clone();
        let lang = self.target_lang.clone();
        tokio::spawn(async move {
            let translated =
                translate_with_cache(&config, &cache, &lang, &sentence).await.unwrap_or_else(|e| {
                    log::warn!("[同声传译] 句段翻译失败: {}", e);
                    String::new()
                });
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
                            if !st.final_translation.is_empty() {
                                st.final_translation.push(' ');
                            }
                            st.final_translation.push_str(&t);
                        }
                        st.applied_seq = next;
                    }
                    None => break,
                }
            }
        });
    }

    fn spawn_provisional(&self, gen: u64, text: String) {
        let state = self.state.clone();
        let config = self.config.clone();
        let cache = self.cache.clone();
        let lang = self.target_lang.clone();
        tokio::spawn(async move {
            let translated =
                translate_with_cache(&config, &cache, &lang, &text).await.unwrap_or_else(|e| {
                    log::debug!("[同声传译] 临时译文翻译失败: {}", e);
                    String::new()
                });
            let mut st = state.lock().await;
            if st.provisional_gen == gen {
                st.provisional = translated;
            }
        });
    }
}

/// 切分完整句段：返回 (完整句段列表, 未完结尾部)
fn split_complete_sentences(text: &str) -> (Vec<String>, String) {
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

/// 带缓存翻译：先查缓存，未命中则调用 LLM 并写入缓存
async fn translate_with_cache(
    config: &Arc<ConfigManager>,
    cache: &Arc<Mutex<HashMap<String, String>>>,
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

    let translated = translate_text(config, target_lang, input).await?;

    let mut c = cache.lock().await;
    if c.len() > 2048 {
        c.clear();
    }
    c.insert(cache_key, translated.clone());
    Ok(translated)
}

/// 调用 OpenAI 兼容 chat/completions 接口翻译一段文本
///
/// 接口配置优先级：字幕翻译专用 LLM 配置 → 「LLM 智能校对」配置 → 内置默认（SiliconFlow）。
pub async fn translate_text(
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

    let chat: ChatResponse = resp.json().await.map_err(|e| anyhow::anyhow!("翻译响应解析失败: {}", e))?;
    let content = chat
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .unwrap_or_default();
    Ok(clean_translation(&content))
}

/// 清理 LLM 输出：去首尾空白/引号/「」/换行
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

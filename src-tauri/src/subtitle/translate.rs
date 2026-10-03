//! 同声传译。
//!
//! - **按目标语言**建工作者：多个字幕窗口选同一语言只翻译一次；
//! - 定稿字幕按 ID 翻译（并发受限、带上文），译文写入状态板并回填转录，乱序到达也不会错位；
//! - 当前行同声预览：每个语言一个节流循环，只翻译最新文本，过期结果按代际丢弃。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::{watch, Mutex, Semaphore};
use tokio::task::JoinHandle;

use crate::api::client::HTTP_CLIENT;
use crate::config::{ConfigManager, SubtitleWindow};
use crate::subtitle::state::{Caption, SharedState};

/// 单次翻译请求超时
const TRANSLATE_TIMEOUT: Duration = Duration::from_secs(15);
/// 同一语言定稿翻译的最大并发
const FINAL_CONCURRENCY: usize = 3;
/// 同声预览：最小请求间隔与合帧去抖
const LIVE_MIN_INTERVAL: Duration = Duration::from_millis(900);
const LIVE_DEBOUNCE: Duration = Duration::from_millis(180);
/// 缓存上限
const CACHE_LIMIT: usize = 2048;

// ==================== 翻译引擎 ====================

#[async_trait]
pub trait TranslationEngine: Send + Sync {
    fn name(&self) -> &'static str;

    /// 翻译 `text`；`context` 为紧邻的上文原文（仅供理解，不翻译）
    async fn translate(&self, config: &ConfigManager, target_lang: &str, text: &str, context: &[String]) -> Result<String>;
}

/// 按配置名解析引擎（None = 关闭或未注册）
pub fn resolve_engine(name: &str) -> Option<Arc<dyn TranslationEngine>> {
    match name {
        "llm" => Some(Arc::new(LlmTranslationEngine)),
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

/// OpenAI 兼容 chat/completions。
/// 端点优先级：字幕同传专用配置 → 「LLM 智能校对」配置 → SiliconFlow 默认。
pub struct LlmTranslationEngine;

const SYSTEM_PROMPT: &str = "你是专业的同声传译员，负责把实时语音识别的文字翻译成目标语言。\
规则：1. 只输出译文本身，不要解释、引号或任何前后缀；\
2. 意思完整、符合目标语言的口语习惯，语音识别的同音错字按上下文理解；\
3. 数字、人名、专有名词保留原样或使用通行译名；\
4. 原文已是目标语言时原样输出；\
5. 「上文」只用于理解语境，绝不要翻译或输出上文。";

#[async_trait]
impl TranslationEngine for LlmTranslationEngine {
    fn name(&self) -> &'static str {
        "llm"
    }

    async fn translate(&self, config: &ConfigManager, target_lang: &str, text: &str, context: &[String]) -> Result<String> {
        let input = text.trim();
        if input.is_empty() {
            return Ok(String::new());
        }
        let (url, key, model) = endpoint(config);
        if key.is_empty() {
            anyhow::bail!("同声传译 LLM API Key 未配置（可在字幕「同传」设置或「设置 → LLM 智能校对」中填写）");
        }

        let mut user = format!("目标语言：{}\n", target_lang);
        if !context.is_empty() {
            user.push_str("\n上文（仅供理解，不要翻译）：\n");
            for c in context {
                user.push_str("- ");
                user.push_str(c);
                user.push('\n');
            }
        }
        user.push_str("\n需要翻译的原文：\n");
        user.push_str(input);

        let body = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": user }
            ],
            "temperature": 0.2,
            "max_tokens": 1024,
            "stream": false,
        });

        let resp = tokio::time::timeout(
            TRANSLATE_TIMEOUT,
            HTTP_CLIENT.post(&url).header("Authorization", format!("Bearer {}", key)).json(&body).send(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("翻译请求超时"))??;

        if !resp.status().is_success() {
            let status = resp.status();
            let brief: String = resp.text().await.unwrap_or_default().chars().take(200).collect();
            anyhow::bail!("翻译接口错误 {}: {}", status, brief);
        }
        let chat: ChatResponse = resp.json().await.map_err(|e| anyhow::anyhow!("翻译响应解析失败: {}", e))?;
        let content = chat.choices.into_iter().next().map(|c| c.message.content).unwrap_or_default();
        Ok(clean_translation(&content))
    }
}

fn endpoint(config: &ConfigManager) -> (String, String, String) {
    let mut url = config.subtitle_translation_llm_api_url();
    let mut key = config.subtitle_translation_llm_api_key();
    let mut model = config.subtitle_translation_llm_model();
    if key.trim().is_empty() {
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
    (url.trim().to_string(), key.trim().to_string(), model.trim().to_string())
}

/// 清理模型输出：去首尾空白、成对引号、常见「译文：」前缀
pub fn clean_translation(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    for prefix in ["译文：", "译文:", "Translation:", "翻译："] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim().to_string();
        }
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() >= 2 {
        let paired = matches!((chars[0], chars[chars.len() - 1]), ('"', '"') | ('“', '”') | ('「', '」') | ('\'', '\''));
        if paired {
            s = chars[1..chars.len() - 1].iter().collect::<String>().trim().to_string();
        }
    }
    s
}

// ==================== 翻译调度 ====================

/// 译文落地回调（会话层用于回填转录并通知主界面）
pub type TranslationSink = Arc<dyn Fn(u64, &str, &str) + Send + Sync>;

type Cache = Arc<Mutex<HashMap<String, String>>>;

struct LangWorker {
    lang: String,
    engine: Arc<dyn TranslationEngine>,
    live_tx: Option<watch::Sender<(u64, String)>>,
    sem: Arc<Semaphore>,
}

/// 一次字幕会话的翻译调度器
pub struct Translator {
    workers: Vec<Arc<LangWorker>>,
    config: Arc<ConfigManager>,
    state: Arc<SharedState>,
    cache: Cache,
    sink: TranslationSink,
    inflight: Arc<AtomicUsize>,
    live_tasks: StdMutex<Vec<JoinHandle<()>>>,
}

impl Translator {
    /// 从启用的窗口构建：按目标语言去重；任一窗口开启同声预览则该语言开启
    pub fn build(config: Arc<ConfigManager>, state: Arc<SharedState>, windows: &[SubtitleWindow], sink: TranslationSink) -> Self {
        let mut langs: Vec<(String, Arc<dyn TranslationEngine>, bool)> = Vec::new();
        for w in windows {
            let Some(engine) = resolve_engine(&w.translation.engine) else { continue };
            let lang = w.translation.target_lang.trim().to_string();
            if lang.is_empty() {
                continue;
            }
            match langs.iter_mut().find(|(l, _, _)| *l == lang) {
                Some(entry) => entry.2 |= w.translation.interim,
                None => langs.push((lang, engine, w.translation.interim)),
            }
        }

        let cache: Cache = Arc::new(Mutex::new(HashMap::new()));
        let mut workers = Vec::new();
        let mut live_tasks = Vec::new();
        {
            let mut board = state.write();
            for (lang, _, _) in &langs {
                board.ensure_lang(lang);
            }
        }
        for (lang, engine, interim) in langs {
            log::info!("[同传] 目标语言 {}（{}，同声预览 {}）", lang, engine.name(), if interim { "开" } else { "关" });
            let live_tx = if interim {
                let (tx, rx) = watch::channel((0u64, String::new()));
                live_tasks.push(tokio::spawn(live_loop(
                    lang.clone(),
                    engine.clone(),
                    config.clone(),
                    state.clone(),
                    cache.clone(),
                    rx,
                )));
                Some(tx)
            } else {
                None
            };
            workers.push(Arc::new(LangWorker { lang, engine, live_tx, sem: Arc::new(Semaphore::new(FINAL_CONCURRENCY)) }));
        }

        Self {
            workers,
            config,
            state,
            cache,
            sink,
            inflight: Arc::new(AtomicUsize::new(0)),
            live_tasks: StdMutex::new(live_tasks),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }

    /// A 源新定稿一行：各语言翻译后写入状态板并回调
    pub fn on_caption(&self, caption: &Caption, context: Vec<String>) {
        for w in &self.workers {
            let worker = w.clone();
            let config = self.config.clone();
            let state = self.state.clone();
            let cache = self.cache.clone();
            let sink = self.sink.clone();
            let inflight = self.inflight.clone();
            let id = caption.id;
            let text = caption.text.clone();
            let context = context.clone();
            inflight.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let result = async {
                    let _permit = worker.sem.acquire().await.ok()?;
                    match translate_cached(&cache, worker.engine.as_ref(), &config, &worker.lang, &text, &context).await {
                        Ok(t) if !t.is_empty() => Some(t),
                        Ok(_) => None,
                        Err(e) => {
                            log::warn!("[同传] 翻译失败（{}）: {}", worker.lang, e);
                            None
                        }
                    }
                }
                .await;
                if let Some(translated) = result {
                    {
                        let mut board = state.write();
                        // 已被裁出最近行的字幕不再写回状态板（转录照常回填）
                        if board.captions.iter().any(|c| c.id == id) {
                            board.translations.entry(worker.lang.clone()).or_default().finals.insert(id, translated.clone());
                        }
                    }
                    state.bump();
                    sink(id, &worker.lang, &translated);
                }
                inflight.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }

    /// A 源当前行变化：交给各语言的节流循环（只保留最新文本）
    pub fn on_live(&self, gen: u64, text: &str) {
        for w in &self.workers {
            if let Some(tx) = &w.live_tx {
                let next = (gen, text.trim().to_string());
                tx.send_if_modified(|cur| {
                    if *cur != next {
                        *cur = next.clone();
                        true
                    } else {
                        false
                    }
                });
            }
        }
    }

    /// 等待在途定稿翻译落地（会话结束时调用，避免最后几句没有译文）
    pub async fn drain(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.inflight.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub fn shutdown(&self) {
        for t in self.live_tasks.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            t.abort();
        }
    }
}

impl Drop for Translator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 同声预览循环：去抖 + 最小间隔，只翻译最新文本；代际不符（该行已定稿）则丢弃
async fn live_loop(
    lang: String,
    engine: Arc<dyn TranslationEngine>,
    config: Arc<ConfigManager>,
    state: Arc<SharedState>,
    cache: Cache,
    mut rx: watch::Receiver<(u64, String)>,
) {
    let mut last_request: Option<Instant> = None;
    loop {
        if rx.changed().await.is_err() {
            break;
        }
        tokio::time::sleep(LIVE_DEBOUNCE).await;
        if let Some(t) = last_request {
            let wait = LIVE_MIN_INTERVAL.saturating_sub(t.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        let (gen, text) = rx.borrow_and_update().clone();
        if text.is_empty() {
            continue;
        }
        last_request = Some(Instant::now());
        let translated = match translate_cached(&cache, engine.as_ref(), &config, &lang, &text, &[]).await {
            Ok(t) if !t.is_empty() => t,
            Ok(_) => continue,
            Err(e) => {
                log::debug!("[同传] 同声预览翻译失败（{}）: {}", lang, e);
                continue;
            }
        };
        let applied = {
            let mut board = state.write();
            if board.live_gen == gen && board.running {
                board.translations.entry(lang.clone()).or_default().live = translated;
                true
            } else {
                false
            }
        };
        if applied {
            state.bump();
        }
    }
}

async fn translate_cached(
    cache: &Cache,
    engine: &dyn TranslationEngine,
    config: &ConfigManager,
    lang: &str,
    text: &str,
    context: &[String],
) -> Result<String> {
    let input = text.trim();
    if input.is_empty() {
        return Ok(String::new());
    }
    let key = format!("{}\u{1}{}", lang, input);
    if let Some(hit) = cache.lock().await.get(&key) {
        return Ok(hit.clone());
    }
    let out = engine.translate(config, lang, input, context).await?;
    let mut c = cache.lock().await;
    if c.len() >= CACHE_LIMIT {
        c.clear();
    }
    c.insert(key, out.clone());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_translation_strips_quotes_and_prefix() {
        assert_eq!(clean_translation("  \"Hello.\"  "), "Hello.");
        assert_eq!(clean_translation("译文：你好"), "你好");
        assert_eq!(clean_translation("“A”"), "A");
        assert_eq!(clean_translation("He said \"hi\""), "He said \"hi\"");
    }
}

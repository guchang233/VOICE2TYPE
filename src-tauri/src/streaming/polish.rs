//! 流式识别 AI 润色（硅基流动 / Groq 免费 Chat 模型）

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;

use crate::api::client::HTTP_CLIENT;
use crate::config::ConfigManager;

const SILICONFLOW_CHAT_URL: &str = "https://api.siliconflow.cn/v1/chat/completions";
const SILICONFLOW_MODEL: &str = "Qwen/Qwen2.5-7B-Instruct";
const GROQ_CHAT_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const GROQ_MODEL: &str = "llama-3.1-8b-instant";

/// 含小数点数字串：0.0.65、3.14、版本号等
static DOT_NUMBER_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\d+(?:\.\d+)+").expect("dot number re"));

const SYSTEM_PROMPT: &str = r#"你是语音转写校对助手。工作原则：宁可不改，也绝不乱改。

硬性规则（违反即失败）：
1. 所有阿拉伯数字 0-9、小数点、版本号、金额、IP、编号必须原样保留，禁止把数字改成字母（例如禁止把 0 改成 o/f，禁止把 1 改成 l）。
2. 禁止合并、省略、概括任何数字或带小数点的片段（如 0.0.65 必须完整保留为 0.0.65）。
3. 仅当你 100% 确定是中文同音错字时才改汉字；数字、英文、符号一律不动。
4. 输出长度不得明显短于原文；不确定则原样输出。
5. 只输出校对正文，不要解释、不要引号。"#;

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

struct PolishBackend<'a> {
    url: &'a str,
    key: String,
    model: &'a str,
}

pub async fn polish_with_ai(text: &str, config: &ConfigManager) -> Result<String> {
    let input = text.trim();
    if input.is_empty() {
        return Ok(String::new());
    }

    // 含版本号/小数等片段时直接跳过 AI，避免 0→f 这类幻觉
    if should_skip_ai_polish(input) {
        return Ok(input.to_string());
    }

    let backend = match resolve_backend(config) {
        Ok(b) => b,
        Err(e) => {
            return Err(e);
        }
    };

    polish_with_custom(input, backend.url, &backend.key, backend.model).await
}

/// 使用自定义 OpenAI 兼容接口做智能校对。
///
/// 与 [`polish_with_ai`] 共享 prompt、跳过策略与安全检查，
/// 但允许调用方指定 `url` / `key` / `model`，供后处理链的 `LlmCorrector` 复用。
///
/// - `url`：chat/completions 端点
/// - `key`：API Key（Bearer 鉴权）
/// - `model`：模型名称
pub async fn polish_with_custom(
    text: &str,
    url: &str,
    key: &str,
    model: &str,
) -> Result<String> {
    let input = text.trim();
    if input.is_empty() {
        return Ok(String::new());
    }

    if should_skip_ai_polish(input) {
        return Ok(input.to_string());
    }

    if key.is_empty() {
        anyhow::bail!("LLM API Key 未配置");
    }

    let user_prompt = format!(
        "极保守校对。数字与小数点必须逐字保留（例如 0.0.65 不能改成 f.0.65 或 0.65）。仅改确定的中文错字。\n\n原文：\n{}",
        input
    );

    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_prompt}
        ],
        "temperature": 0.0,
        "max_tokens": 2048,
    });

    let resp = match HTTP_CLIENT
        .post(url)
        .header("Authorization", format!("Bearer {}", key))
        .json(&body)
        .send()
        .await
        .context("AI 润色请求失败")
    {
        Ok(r) => r,
        Err(e) => {
            return Err(e);
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let err = resp.text().await.unwrap_or_default();
        anyhow::bail!("AI 润色 HTTP {}: {}", status, err);
    }

    let parsed: ChatResponse = match resp
        .json()
        .await
        .context("解析 AI 润色响应失败")
    {
        Ok(p) => p,
        Err(e) => {
            return Err(e);
        }
    };
    let mut out = parsed
        .choices
        .first()
        .map(|c| c.message.content.trim().to_string())
        .unwrap_or_default();

    out = strip_wrapping_quotes(&out);

    if out.is_empty() {
        anyhow::bail!("AI 润色返回为空");
    }

    if !polish_is_safe(input, &out) {
        anyhow::bail!("AI 润色改动数字或压缩过度，已保留原文");
    }

    Ok(out)
}

/// 错别字修正专用 system prompt。
///
/// 与 `polish_with_custom` 的保守 prompt 不同，此 prompt 明确鼓励 LLM
/// 主动修正 ASR 同音错字并补全标点，同时保留数字安全规则。
const CORRECT_SYSTEM_PROMPT: &str = r#"你是语音识别（ASR）转写文本的校对专家。你的核心任务是修正 ASR 产生的同音错字，并补全缺失的标点。

ASR 常见同音错字示例（必须主动修正类似错误）：
- "后处里" → "后处理"（里→理）
- "流氏输出" → "流式输出"（氏→式）
- "断剧测试" → "断句测试"（剧→句）
- "因该" → "应该"
- "工做" → "工作"
- "记地" → "记得"
- "气份" → "气氛"
- "刻服" → "克服"

工作规则：
1. 主动修正中文同音错字、形近错字，使语义通顺。不确定时根据上下文选择最合理的字。
2. 补全缺失的标点（逗号、句号等），使句子完整可读。
3. 阿拉伯数字 0-9、小数点、版本号、金额、IP、URL、英文单词必须原样保留，禁止改动。
4. 不要增删实质内容，不要概括或改写句意，保持原话长度。有时当用户会说一些你无法理解的内容，禁止改动。记住你只改错别字、同音字。
5. 只输出校对后的正文，不要解释、不要引号。"#;

/// 使用自定义 OpenAI 兼容接口做错别字修正。
///
/// 与 [`polish_with_custom`] 的区别：
/// - 使用 [`CORRECT_SYSTEM_PROMPT`]，鼓励 LLM 主动修正同音错字
/// - 复用 [`polish_is_safe`] 数字安全检查作为兜底
///
/// `system_prompt` 为空时使用内置 [`CORRECT_SYSTEM_PROMPT`]，
/// 非空时使用用户自定义 prompt，允许用户调整校对风格。
///
/// 供后处理链的 `LlmCorrector` 调用。
pub async fn correct_with_custom(
    text: &str,
    url: &str,
    key: &str,
    model: &str,
    system_prompt: &str,
) -> Result<String> {
    let input = text.trim();
    if input.is_empty() {
        return Ok(String::new());
    }

    if should_skip_ai_polish(input) {
        return Ok(input.to_string());
    }

    if key.is_empty() {
        anyhow::bail!("LLM API Key 未配置");
    }

    let user_prompt = format!(
        "请校对以下 ASR 转写文本，修正同音错字并补全标点。数字和英文必须原样保留。\n\n原文：\n{}",
        input
    );

    // 用户自定义 prompt 优先；为空则使用内置默认
    let sys = if system_prompt.trim().is_empty() {
        CORRECT_SYSTEM_PROMPT
    } else {
        system_prompt
    };

    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "system", "content": sys},
            {"role": "user", "content": user_prompt}
        ],
        "temperature": 0.1,
        "max_tokens": 4096,
    });

    let resp = match HTTP_CLIENT
        .post(url)
        .header("Authorization", format!("Bearer {}", key))
        .json(&body)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .context("错别字校对请求失败")
    {
        Ok(r) => r,
        Err(e) => {
            return Err(e);
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let err = resp.text().await.unwrap_or_default();
        anyhow::bail!("错别字校对 HTTP {}: {}", status, err);
    }

    let parsed: ChatResponse = match resp
        .json()
        .await
        .context("解析错别字校对响应失败")
    {
        Ok(p) => p,
        Err(e) => {
            return Err(e);
        }
    };
    let raw = parsed
        .choices
        .first()
        .map(|c| c.message.content.trim().to_string())
        .unwrap_or_default();

    // 鲁棒清洗：去代码块、前缀、引号、多余空白
    let out = clean_llm_output(&raw);
    log::debug!("[correct] LLM 原始返回: {:?} -> 清洗后: {:?}", &raw[..raw.len().min(80)], &out[..out.len().min(80)]);

    if out.is_empty() {
        anyhow::bail!("错别字校对返回为空");
    }

    if !polish_is_safe(input, &out) {
        log::warn!("[correct] LLM 改动数字或压缩过度，保留原文");
        return Ok(input.to_string());
    }

    Ok(out)
}

/// 数字/小数/版本号密集或含 0.0.65 这类片段 → 不调用模型
pub fn should_skip_ai_polish_public(text: &str) -> bool {
    should_skip_ai_polish(text)
}

fn should_skip_ai_polish(text: &str) -> bool {
    if DOT_NUMBER_RE.is_match(text) {
        return true;
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return false;
    }
    let digit_or_dot = chars
        .iter()
        .filter(|c| c.is_ascii_digit() || **c == '.')
        .count();
    digit_or_dot * 100 / chars.len() >= 20
}

pub(crate) fn polish_is_safe(original: &str, polished: &str) -> bool {
    let orig_len = original.chars().count();
    let new_len = polished.chars().count();
    if orig_len == 0 {
        return true;
    }

    if orig_len >= 8 && new_len * 100 / orig_len < 80 {
        return false;
    }

    // 阿拉伯数字序列必须完全一致（顺序、个数）
    let orig_digits = ascii_digits(original);
    let new_digits = ascii_digits(polished);
    if !orig_digits.is_empty() && orig_digits != new_digits {
        return false;
    }

    // 每个「数字.数字...」片段必须原样出现在润色结果中
    for token in DOT_NUMBER_RE.find_iter(original) {
        if !polished.contains(token.as_str()) {
            return false;
        }
    }

    let orig_nums = count_cjk_numerals(original);
    let new_nums = count_cjk_numerals(polished);
    if orig_nums >= 4 && new_nums + 1 < orig_nums {
        return false;
    }

    true
}

fn ascii_digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn count_cjk_numerals(s: &str) -> usize {
    s.chars()
        .filter(|c| {
            matches!(
                c,
                '零' | '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十'
                    | '百' | '千' | '万' | '两' | '〇'
            )
        })
        .count()
}

pub(crate) fn strip_wrapping_quotes(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"')) || (t.starts_with('「') && t.ends_with('」')) {
        t[1..t.len().saturating_sub(1)].trim().to_string()
    } else {
        t.to_string()
    }
}

/// 鲁棒清洗 LLM 返回，处理各种不规范输出。
///
/// 处理顺序：
/// 1. trim 首尾空白
/// 2. 去除 Markdown 代码块包裹（```...``` 或 ```text\n...\n```）
/// 3. 去除常见前缀（"校对结果："、"修改后："、"结果："、"答：" 等）
/// 4. 去除外层引号（中英文、书名号）
/// 5. 合并多余空白为单个空格，保持换行
/// 6. 再次 trim
///
/// 保留内部标点和换行结构，只清除外层包装。
pub(crate) fn clean_llm_output(s: &str) -> String {
    let mut t = s.trim().to_string();

    // 1. 去除 Markdown 代码块包裹
    if t.starts_with("```") {
        // 去掉开头的 ``` 或 ```lang
        if let Some(idx) = t.find('\n') {
            let first_line = &t[..idx];
            if first_line.trim().starts_with("```") {
                t = t[idx + 1..].to_string();
                // 去掉结尾的 ```
                if t.ends_with("```") {
                    t = t[..t.len() - 3].to_string();
                }
            }
        } else {
            // 整体就是 ```xxx
            t = t.trim_start_matches("```").to_string();
        }
        t = t.trim().to_string();
    }

    // 2. 去除常见前缀（LLM 有时会加"校对结果："等说明）
    let prefixes: &[&str] = &[
        "校对结果：", "校对结果:", "校对后：", "校对后:",
        "修改后：", "修改后:", "修改结果：", "修改结果:",
        "结果：", "结果:", "答：", "答:",
        "输出：", "输出:", "正文：", "正文:",
    ];
    for p in prefixes {
        if t.starts_with(p) {
            t = t[p.len()..].trim().to_string();
            break;
        }
    }

    // 3. 去除外层引号（中英文双引号、书名号）
    t = strip_wrapping_quotes(&t);
    // 英文双引号
    if t.starts_with('"') && t.ends_with('"') && t.len() > 1 {
        t = t[1..t.len() - 1].trim().to_string();
    }
    if t.starts_with('“') && t.ends_with('”') && t.len() > 1 {
        t = t[1..t.len() - 1].trim().to_string();
    }

    // 4. 合并连续空格为单个（保留换行）
    let mut result = String::with_capacity(t.len());
    let mut prev_space = false;
    for c in t.chars() {
        if c == ' ' {
            if !prev_space {
                result.push(c);
            }
            prev_space = true;
        } else {
            result.push(c);
            prev_space = false;
        }
    }

    result.trim().to_string()
}

fn resolve_backend(config: &ConfigManager) -> Result<PolishBackend<'_>> {
    let sf = config.get_siliconflow_api_key();
    if !sf.is_empty() {
        return Ok(PolishBackend {
            url: SILICONFLOW_CHAT_URL,
            key: sf,
            model: SILICONFLOW_MODEL,
        });
    }
    let groq = config.get_groq_api_key();
    if !groq.is_empty() {
        return Ok(PolishBackend {
            url: GROQ_CHAT_URL,
            key: groq,
            model: GROQ_MODEL,
        });
    }
    anyhow::bail!("未配置硅基流动或 Groq API Key，无法使用 AI 润色")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_version_like() {
        assert!(should_skip_ai_polish("版本 0.0.65"));
    }

    #[test]
    fn reject_digit_change() {
        assert!(!polish_is_safe("0.0.65", "f.0.65"));
        assert!(polish_is_safe("0.0.65", "0.0.65"));
    }
}

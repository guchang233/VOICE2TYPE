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

    let backend = resolve_backend(config)?;
    let user_prompt = format!(
        "极保守校对。数字与小数点必须逐字保留（例如 0.0.65 不能改成 f.0.65 或 0.65）。仅改确定的中文错字。\n\n原文：\n{}",
        input
    );

    let body = serde_json::json!({
        "model": backend.model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_prompt}
        ],
        "temperature": 0.0,
        "max_tokens": 2048,
    });

    let resp = HTTP_CLIENT
        .post(backend.url)
        .header("Authorization", format!("Bearer {}", backend.key))
        .json(&body)
        .send()
        .await
        .context("AI 润色请求失败")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let err = resp.text().await.unwrap_or_default();
        anyhow::bail!("AI 润色 HTTP {}: {}", status, err);
    }

    let parsed: ChatResponse = resp.json().await.context("解析 AI 润色响应失败")?;
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

/// 数字/小数/版本号密集或含 0.0.65 这类片段 → 不调用模型
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

fn polish_is_safe(original: &str, polished: &str) -> bool {
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

fn strip_wrapping_quotes(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('"') && t.ends_with('"')) || (t.starts_with('「') && t.ends_with('」')) {
        t[1..t.len().saturating_sub(1)].trim().to_string()
    } else {
        t.to_string()
    }
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

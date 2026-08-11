//! Token 计数器：估算字符串和消息列表的 token 数。
//!
//! 估算策略：
//! - CJK 字符约 1.5 tokens/字符
//! - 非 CJK（ASCII 字母/数字/标点）约 0.25 tokens/字符（约 4 字符/token）
//! - 空格不计入；连续 ASCII 与 CJK 混合时按字符分类分别估算
//!
//! 消息列表额外计入每条消息的角色/分隔固定开销（约 2 tokens/条），
//! 使估算更接近真实 tokenizer 行为（role 字段本身也占用 token）。

use crate::llm::LlmMessage;

/// 每条消息的角色/分隔等固定开销。
const ROLE_OVERHEAD_TOKENS: usize = 2;

/// 判断字符是否为 CJK 字符（中文/日文/韩文）。
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}' |
        '\u{3400}'..='\u{4DBF}' |
        '\u{3000}'..='\u{303F}' |
        '\u{3040}'..='\u{309F}' |
        '\u{30A0}'..='\u{30FF}' |
        '\u{AC00}'..='\u{D7AF}'
    )
}

/// Token 计数器。无状态，所有方法都是关联函数。
pub struct TokenCounter;

impl TokenCounter {
    /// 估算字符串的 token 数。
    ///
    /// 逐字符分类：CJK 按 1.5/字符，非 CJK 按 0.25/字符（连续段向上取整），
    /// 空格不计。空字符串返回 1，避免除零场景。
    pub fn estimate(text: &str) -> usize {
        let mut tokens = 0.0f64;
        let mut ascii_run = 0usize;

        for c in text.chars() {
            if is_cjk(c) {
                // 先结算累计的 ASCII 段
                tokens += ascii_run as f64 * 0.25;
                ascii_run = 0;
                tokens += 1.5;
            } else if c.is_whitespace() {
                tokens += ascii_run as f64 * 0.25;
                ascii_run = 0;
            } else {
                ascii_run += 1;
            }
        }
        tokens += ascii_run as f64 * 0.25;

        (tokens.ceil() as usize).max(1)
    }

    /// 估算整个消息列表的 token 总数。
    ///
    /// 每条消息计入 `ROLE_OVERHEAD_TOKENS` 固定开销（role 字段与分隔符）。
    pub fn estimate_messages(messages: &[LlmMessage]) -> usize {
        messages
            .iter()
            .map(|m| {
                let content_tokens = Self::estimate(m.content.as_deref().unwrap_or(""));
                let tool_calls_tokens = m
                    .tool_calls
                    .as_ref()
                    .and_then(|tc| serde_json::to_string(tc).ok())
                    .map(|s| Self::estimate(&s))
                    .unwrap_or(0);
                ROLE_OVERHEAD_TOKENS + content_tokens + tool_calls_tokens
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmMessage;

    #[test]
    fn empty_string_returns_min_one() {
        // 至少返回 1，避免 0 token 的除零场景
        assert_eq!(TokenCounter::estimate(""), 1);
    }

    #[test]
    fn ascii_words_use_025_ratio() {
        // "hello world": 2 words × 5 chars/word × 0.25 = ceil(1.25) × 2 ≈ 4 tokens
        let tokens = TokenCounter::estimate("hello world");
        assert!((3..=5).contains(&tokens), "expected ~4 tokens, got {}", tokens);
    }

    #[test]
    fn cjk_chars_use_1_5_ratio() {
        // 4 CJK chars × 1.5 = 6 tokens（每个汉字当 1 word）
        let tokens = TokenCounter::estimate("你好世界");
        assert_eq!(tokens, 6);
    }

    #[test]
    fn mixed_cjk_ascii_sums_separately() {
        // CJK 部分按 1.5/字符，ASCII 部分按 0.25/字符
        let tokens = TokenCounter::estimate("Hello 世界");
        assert!(tokens > 4, "mixed text should have more tokens than pure ascii, got {}", tokens);
    }

    #[test]
    fn estimate_messages_sums_all() {
        let msgs = vec![
            LlmMessage {
                role: "user".to_string(),
                content: Some("Hello".to_string()),
                tool_calls: None,
                tool_call_id: None,
            },
            LlmMessage {
                role: "assistant".to_string(),
                content: Some("World".to_string()),
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        let total = TokenCounter::estimate_messages(&msgs);
        let sum = TokenCounter::estimate("Hello") + TokenCounter::estimate("World");
        // 每条消息计入 ROLE_OVERHEAD_TOKENS（2）固定开销
        assert_eq!(total, sum + ROLE_OVERHEAD_TOKENS * msgs.len());
    }

    #[test]
    fn estimate_messages_handles_none_content() {
        let msgs = vec![LlmMessage {
            role: "assistant".to_string(),
            content: None, // tool_calls-only message
            tool_calls: None,
            tool_call_id: None,
        }];
        // None content 通过 unwrap_or("") 得到空字符串，estimate("") 返回 .max(1) = 1
        // 加上角色固定开销 = 3
        assert_eq!(
            TokenCounter::estimate_messages(&msgs),
            ROLE_OVERHEAD_TOKENS + 1
        );
    }

    #[test]
    fn estimate_messages_includes_tool_calls() {
        let msgs = vec![LlmMessage {
            role: "assistant".to_string(),
            content: None,
            tool_calls: Some(vec![crate::llm::ToolCall {
                id: "call_123".to_string(),
                function: crate::llm::ToolCallFunction {
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({ "file_path": "src/main.rs" }),
                },
            }]),
            tool_call_id: None,
        }];
        let total = TokenCounter::estimate_messages(&msgs);
        assert!(total > 1, "tool_calls should contribute to token count, got {}", total);
    }
}

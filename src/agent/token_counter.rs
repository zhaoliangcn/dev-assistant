//! Token 计数器：估算字符串和消息列表的 token 数。
//!
//! 估算策略：
//! - CJK 字符约 2 tokens/字符
//! - 非 CJK 单词约 0.75 tokens/字符

use crate::llm::LlmMessage;

/// Token 计数器。无状态，所有方法都是关联函数。
pub struct TokenCounter;

impl TokenCounter {
    /// 估算字符串的 token 数。
    pub fn estimate(text: &str) -> usize {
        let mut tokens = 0usize;

        for word in text.split_whitespace() {
            let is_cjk = word.chars().any(|c| {
                matches!(c,
                    '\u{4E00}'..='\u{9FFF}' |
                    '\u{3400}'..='\u{4DBF}' |
                    '\u{3000}'..='\u{303F}' |
                    '\u{3040}'..='\u{309F}' |
                    '\u{30A0}'..='\u{30FF}' |
                    '\u{AC00}'..='\u{D7AF}'
                )
            });

            if is_cjk {
                tokens += word.chars().count() * 2;
            } else {
                tokens += (word.chars().count() as f64 * 0.75).ceil() as usize;
            }
        }

        tokens.max(1)
    }

    /// 估算整个消息列表的 token 总数。
    pub fn estimate_messages(messages: &[LlmMessage]) -> usize {
        messages
            .iter()
            .map(|m| Self::estimate(m.content.as_deref().unwrap_or("")))
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
    fn ascii_words_use_075_ratio() {
        // "hello world": 2 words × 5 chars/word × 0.75 = ceil(3.75) × 2 ≈ 8 tokens
        let tokens = TokenCounter::estimate("hello world");
        assert!(tokens >= 5 && tokens <= 8, "expected ~6 tokens, got {}", tokens);
    }

    #[test]
    fn cjk_chars_use_2_ratio() {
        // 4 CJK chars × 2 = 8 tokens（每个汉字当 1 word）
        let tokens = TokenCounter::estimate("你好世界");
        assert_eq!(tokens, 8);
    }

    #[test]
    fn mixed_cjk_ascii_sums_separately() {
        // CJK 部分按 2/字符，ASCII 部分按 0.75/字符
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
        assert_eq!(total, sum);
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
        assert_eq!(TokenCounter::estimate_messages(&msgs), 1);
    }
}

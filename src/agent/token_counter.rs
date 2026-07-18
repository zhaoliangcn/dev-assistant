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

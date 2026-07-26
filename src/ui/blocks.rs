//! 消息块类型定义

use serde_json::Value;

/// 消息块类型
#[derive(Debug, Clone)]
#[allow(dead_code)] // reserved for future block-based UI rendering
pub enum MessageBlock {
    /// 用户消息
    User {
        content: String,
    },
    /// 助手消息
    #[allow(dead_code)]
    Assistant {
        content: String,
        is_streaming: bool,
    },
    /// 思考状态
    Thinking {
        content: String,
    },
    /// 工具调用
    #[allow(dead_code)]
    ToolCall {
        tool_name: String,
        args: Value,
    },
    /// 工具执行结果
    ToolResult {
        #[allow(dead_code)]
        tool_name: String,
        success: bool,
        content: String,
    },
    /// 系统消息
    System {
        content: String,
    },
    /// 错误消息
    Error {
        content: String,
    },
    /// 分隔线
    Divider,
}

impl MessageBlock {
    /// 获取块的渲染前缀
    pub fn prefix(&self) -> &'static str {
        match self {
            MessageBlock::User { .. } => "👤 你",
            MessageBlock::Assistant { .. } => "🤖 助手",
            MessageBlock::Thinking { .. } => "💭 思考",
            MessageBlock::ToolCall { .. } => "🔧 调用",
            MessageBlock::ToolResult { success, .. } => {
                if *success { "✅ 结果" } else { "❌ 失败" }
            }
            MessageBlock::System { .. } => "⚙ 系统",
            MessageBlock::Error { .. } => "🔥 错误",
            MessageBlock::Divider => "",
        }
    }
    
    /// 获取块的角色标签（用于 display_messages 兼容）
    #[allow(dead_code)] // reserved for future block-based UI rendering
    pub fn role_label(&self) -> &'static str {
        match self {
            MessageBlock::User { .. } => "你",
            MessageBlock::Assistant { .. } => "助手",
            MessageBlock::Thinking { .. } => "思考",
            MessageBlock::ToolCall { .. } => "工具",
            MessageBlock::ToolResult { success, .. } => {
                if *success { "成功" } else { "错误" }
            }
            MessageBlock::System { .. } => "系统",
            MessageBlock::Error { .. } => "错误",
            MessageBlock::Divider => "分隔线",
        }
    }
    
    /// 获取块的内容（用于渲染）
    pub fn content(&self) -> String {
        match self {
            MessageBlock::User { content } => content.clone(),
            MessageBlock::Assistant { content, .. } => content.clone(),
            MessageBlock::Thinking { content } => content.clone(),
            MessageBlock::ToolCall { tool_name, args } => {
                format!(
                    "\x1b[38;2;156;189;248m{}\x1b[0m\n{}",
                    tool_name,
                    serde_json::to_string_pretty(args).unwrap_or_default()
                )
            }
            MessageBlock::ToolResult { content, .. } => content.clone(),
            MessageBlock::System { content } => content.clone(),
            MessageBlock::Error { content } => content.clone(),
            MessageBlock::Divider => String::new(),
        }
    }
    
    /// 判断块是否需要 Markdown 渲染
    pub fn needs_markdown(&self) -> bool {
        matches!(
            self,
            MessageBlock::Assistant { .. } | MessageBlock::ToolResult { .. }
        )
    }
}

/// 从 (role, content) 元组构造 MessageBlock。
///
/// 用于将旧式 `(String, String)` 消息列表转换为块类型，
/// 兼容现有 display_messages 格式。
impl From<(&str, &str)> for MessageBlock {
    fn from((role, content): (&str, &str)) -> Self {
        if role.starts_with("▸ 你") || role == "你" || role == "👤 你" || role == "user" {
            MessageBlock::User { content: content.to_string() }
        } else if role.starts_with("◂ 助手") || role == "助手" || role == "🤖 助手" || role == "assistant" {
            MessageBlock::Assistant { content: content.to_string(), is_streaming: false }
        } else if role.starts_with("💭") || role == "思考" || role == "thinking" {
            MessageBlock::Thinking { content: content.to_string() }
        } else if role.starts_with("▸ 成功") || role == "成功" || role == "✅ 结果" || role == "success" {
            MessageBlock::ToolResult {
                tool_name: String::new(),
                success: true,
                content: content.to_string(),
            }
        } else if role.starts_with("▸ 错误") || role == "错误" || role == "❌ 失败" || role == "🔥 错误" || role == "error" {
            MessageBlock::Error { content: content.to_string() }
        } else if role.starts_with("▸ 警告") || role == "警告" || role == "⚠️ 警告" || role == "warning" {
            MessageBlock::System { content: content.to_string() }
        } else if role.starts_with("🔧") || role == "工具" || role == "tool" {
            MessageBlock::ToolCall {
                tool_name: role.trim_start_matches("🔧 ").to_string(),
                args: serde_json::Value::Null,
            }
        } else {
            MessageBlock::System { content: content.to_string() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_user() {
        let block = MessageBlock::from(("你", "hello"));
        assert!(matches!(block, MessageBlock::User { .. }));
        assert_eq!(block.content(), "hello");
    }

    #[test]
    fn test_from_assistant() {
        let block = MessageBlock::from(("助手", "world"));
        assert!(matches!(block, MessageBlock::Assistant { .. }));
        assert_eq!(block.content(), "world");
    }

    #[test]
    fn test_from_error() {
        let block = MessageBlock::from(("错误", "something broke"));
        assert!(matches!(block, MessageBlock::Error { .. }));
    }

    #[test]
    fn test_from_success() {
        let block = MessageBlock::from(("成功", "done"));
        assert!(matches!(block, MessageBlock::ToolResult { .. }));
        if let MessageBlock::ToolResult { success, .. } = block {
            assert!(success);
        }
    }

    #[test]
    fn test_from_thinking() {
        let block = MessageBlock::from(("💭", "analyzing..."));
        assert!(matches!(block, MessageBlock::Thinking { .. }));
    }

    #[test]
    fn test_from_unknown_falls_back_to_system() {
        let block = MessageBlock::from(("unknown_role", "data"));
        assert!(matches!(block, MessageBlock::System { .. }));
    }
}

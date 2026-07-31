//! 消息块类型定义

use serde_json::Value;

/// 消息块类型
#[derive(Debug, Clone)]
pub enum MessageBlock {
    /// 用户消息
    User {
        content: String,
    },
    /// 助手消息
    Assistant {
        content: String,
        #[allow(dead_code)]
        is_streaming: bool,
    },
    /// 思考状态
    Thinking {
        content: String,
    },
    /// 工具调用
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
    /// 文件 diff 修改（unified diff 格式，支持绿/红/青色渲染）
    Diff {
        file_path: String,
        diff_content: String,
        summary: Option<String>,
    },
    /// 分隔线
    #[allow(dead_code)]
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
            MessageBlock::Diff { .. } => "📝 修改",
            MessageBlock::Divider => "",
        }
    }

    /// 获取块前缀的状态色（用于渲染时包裹前缀文本）。
    ///
    /// 工具调用 → `tool_fg`，成功结果 → `success_fg`，
    /// 失败结果/错误 → `error_fg`，其余 → `muted_fg`。
    /// 色值来自当前主题（亮/暗自适应），见 [`crate::ui::theme`]。
    #[allow(dead_code)]
    pub fn status_color(&self) -> &'static str {
        let theme = crate::ui::theme::active_theme();
        match self {
            MessageBlock::ToolCall { .. } => theme.tool_fg,
            MessageBlock::ToolResult { success: true, .. } => theme.success_fg,
            MessageBlock::ToolResult { success: false, .. } => theme.error_fg,
            MessageBlock::Error { .. } => theme.error_fg,
            _ => theme.muted_fg,
        }
    }

    /// 获取块的渲染前缀（含文件路径，用于渲染时的完整标题）
    pub fn full_prefix(&self) -> String {
        match self {
            MessageBlock::Diff { file_path, summary, .. } => {
                let base = format!("📝 修改 {}", file_path);
                if let Some(s) = summary {
                    format!("{} — {}", base, s)
                } else {
                    base
                }
            }
            _ => self.prefix().to_string(),
        }
    }

    /// 获取块的角色标签（用于 display_messages 兼容）
    #[allow(dead_code)]
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
            MessageBlock::Diff { .. } => "修改",
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
                let summary = Self::summarize_tool_args(tool_name, args);
                let theme = crate::ui::theme::active_theme();
                format!(
                    "{}{}{}\n{}",
                    theme.tool_fg,
                    tool_name,
                    crate::ui::theme::RESET,
                    summary
                )
            }
            MessageBlock::ToolResult { content, .. } => content.clone(),
            MessageBlock::System { content } => content.clone(),
            MessageBlock::Error { content } => content.clone(),
            MessageBlock::Diff { diff_content, .. } => {
                // 包装为 diff 代码块，让 MarkdownRenderer 拾取并应用绿/红配色
                format!("```diff\n{}\n```", diff_content.trim())
            }
            MessageBlock::Divider => String::new(),
        }
    }

    /// 智能摘要工具调用参数。
    ///
    /// 对常见工具提取关键字段以简洁显示，减少视觉噪音。
    /// 不识别的工具回退到完整的 JSON pretty-print。
    pub fn summarize_tool_args(tool_name: &str, args: &serde_json::Value) -> String {
        let obj = match args.as_object() {
            Some(o) => o,
            None => return serde_json::to_string_pretty(args).unwrap_or_default(),
        };

        match tool_name {
            // ── 文件编辑 ──
            "edit_file" => {
                let path = obj.get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let old_len = obj.get("old_string")
                    .and_then(|v| v.as_str())
                    .map(|s| s.len())
                    .unwrap_or(0);
                let new_len = obj.get("new_string")
                    .and_then(|v| v.as_str())
                    .map(|s| s.len())
                    .unwrap_or(0);
                format!(
                    "  文件: {}\n  变更: 替换 {} 字符 → {} 字符",
                    path, old_len, new_len
                )
            }
            // ── 文件写入 ──
            "write_file" => {
                let path = obj.get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let content_len = obj.get("content")
                    .and_then(|v| v.as_str())
                    .map(|s| s.len())
                    .unwrap_or(0);
                let summary = obj.get("content")
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        let first_line = s.lines().next().unwrap_or("");
                        if first_line.len() > 60 {
                            let truncated: String = first_line.chars().take(60).collect();
                            format!("{}...", truncated)
                        } else {
                            first_line.to_string()
                        }
                    })
                    .unwrap_or_default();
                format!(
                    "  文件: {}\n  大小: {} 字节\n  首行: {}",
                    path, content_len, summary
                )
            }
            // ── 文件读取 ──
            "read_file" => {
                let path = obj.get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let offset = obj.get("offset").and_then(|v| v.as_i64());
                let limit = obj.get("limit").and_then(|v| v.as_i64());
                let range = match (offset, limit) {
                    (Some(o), Some(l)) => format!(" (偏移 {}，限制 {} 行)", o, l),
                    (Some(o), None) => format!(" (从第 {} 行开始)", o),
                    (None, Some(l)) => format!(" (限制 {} 行)", l),
                    (None, None) => String::new(),
                };
                format!("  文件: {}{}", path, range)
            }
            // ── Bash ──
            "bash" => {
                let cmd = obj.get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let truncated = if cmd.len() > 100 {
                    let truncated: String = cmd.chars().take(100).collect();
                    format!("{}... ({} 字符)", truncated, cmd.len())
                } else {
                    cmd.to_string()
                };
                format!("  命令: {}", truncated)
            }
            // ── 搜索 ──
            "grep" | "search" => {
                let pattern = obj.get("pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let path = obj.get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or(".");
                format!("  模式: {}\n  路径: {}", pattern, path)
            }
            "glob" => {
                let pattern = obj.get("pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let path = obj.get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or(".");
                format!("  模式: {}\n  路径: {}", pattern, path)
            }
            // ── 其他工具：回退到 JSON ──
            _ => serde_json::to_string_pretty(args).unwrap_or_default(),
        }
    }

    /// 判断块是否需要 Markdown 渲染
    pub fn needs_markdown(&self) -> bool {
        matches!(
            self,
            MessageBlock::Assistant { .. }
                | MessageBlock::ToolResult { .. }
                | MessageBlock::Diff { .. }
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
        } else if role == "修改" {
            MessageBlock::Diff {
                file_path: String::new(),
                diff_content: content.to_string(),
                summary: None,
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
        } else if role.starts_with("📝") || role == "diff" {
            MessageBlock::Diff {
                file_path: String::new(),
                diff_content: content.to_string(),
                summary: None,
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

    #[test]
    fn test_from_diff_role() {
        let block = MessageBlock::from(("修改", "+new line\n-old line"));
        assert!(matches!(block, MessageBlock::Diff { .. }));
        if let MessageBlock::Diff { diff_content, .. } = block {
            assert!(diff_content.contains("+new line"));
            assert!(diff_content.contains("-old line"));
        }
    }

    #[test]
    fn test_from_diff_marker() {
        let block = MessageBlock::from(("📝 src/main.rs", "diff content"));
        assert!(matches!(block, MessageBlock::Diff { .. }));
    }

    #[test]
    fn test_diff_needs_markdown() {
        let block = MessageBlock::Diff {
            file_path: "test.rs".into(),
            diff_content: "+added".into(),
            summary: None,
        };
        assert!(block.needs_markdown(), "Diff blocks should use Markdown rendering");
    }

    #[test]
    fn test_diff_content_wraps_as_diff_code_block() {
        let block = MessageBlock::Diff {
            file_path: "test.rs".into(),
            diff_content: "+fn new() {}".into(),
            summary: None,
        };
        let content = block.content();
        assert!(content.starts_with("```diff"), "Should be wrapped in diff code block");
        assert!(content.contains("+fn new() {}"), "Original content preserved");
        assert!(content.ends_with("```"), "Should end with closing fence");
    }

    #[test]
    fn test_diff_prefix() {
        let block = MessageBlock::Diff {
            file_path: "src/main.rs".into(),
            diff_content: String::new(),
            summary: None,
        };
        assert_eq!(block.prefix(), "📝 修改");
    }

    #[test]
    fn test_diff_full_prefix() {
        let block = MessageBlock::Diff {
            file_path: "src/main.rs".into(),
            diff_content: String::new(),
            summary: Some("添加新功能".into()),
        };
        let full = block.full_prefix();
        assert!(full.contains("src/main.rs"));
        assert!(full.contains("添加新功能"));
    }
}

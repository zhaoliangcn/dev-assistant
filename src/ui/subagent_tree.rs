//! 子代理树可视化：显示当前运行的子代理层级关系。
//!
//! 格式：
//! ```
//! 🌳 代理树
//! ├── 🧠 Agent (main) [depth 0]
//! │   ├── 🏗 Architect [depth 1] — 架构设计完成 ✅
//! │   ├── 💻 Implementer [depth 1] — 代码实现 ⏳
//! │   └── 🔍 Reviewer [depth 1] — 代码审查 ⬜
//! └── 🧪 Tester [depth 1] — 测试验证 ⏳
//! ```

use std::io::{self, IsTerminal, Write};

use crate::ui::theme::active_theme;

/// 子代理信息。
#[derive(Debug, Clone)]
pub struct SubagentInfo {
    pub name: String,
    pub depth: usize,
    pub agent_type: String,
    pub status: SubagentStatus,
}

#[derive(Debug, Clone)]
pub enum SubagentStatus {
    Running,
    Done,
    Pending,
    Failed,
}

impl SubagentStatus {
    fn label(&self) -> &'static str {
        match self {
            SubagentStatus::Running => "⏳",
            SubagentStatus::Done => "✅",
            SubagentStatus::Pending => "⬜",
            SubagentStatus::Failed => "❌",
        }
    }
}

/// 渲染子代理树。
///
/// `subagents` 是子代理信息列表，`parent_name` 是主代理名称。
pub fn render_subagent_tree(subagents: &[SubagentInfo], parent_name: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        return Ok(());
    }

    let theme = active_theme();

    writeln!(stdout, "\r")?;
    writeln!(stdout, "  🌳 代理树")?;

    // 主代理行
    let main_line = format!(
        "  ├── {}🧠 {} (main) [depth 0]{}",
        theme.tool_fg,
        parent_name,
        crate::ui::theme::RESET
    );
    writeln!(stdout, "{}", main_line)?;

    // 子代理
    if subagents.is_empty() {
        writeln!(stdout, "  └── 无运行中的子代理")?;
    } else {
        let indent = "  │   ";
        let last_idx = subagents.len() - 1;

        for (i, agent) in subagents.iter().enumerate() {
            let connector = if i == last_idx {
                "└── "
            } else {
                "├── "
            };

            let status_color = match agent.status {
                SubagentStatus::Running => theme.tool_fg,
                SubagentStatus::Done => theme.success_fg,
                SubagentStatus::Pending => theme.muted_fg,
                SubagentStatus::Failed => theme.error_fg,
            };

            let line = format!(
                "{}{}{}🏗 {} [depth {}] — {} {}",
                indent,
                connector,
                status_color,
                agent.agent_type,
                agent.depth,
                agent.name,
                agent.status.label(),
            );
            writeln!(stdout, "{}", line)?;
        }
    }

    stdout.flush()?;
    Ok(())
}

/// 简单的树形文本渲染（纯字符串，无 IO），用于测试。
pub fn render_subagent_tree_to_string(subagents: &[SubagentInfo], parent_name: &str) -> String {
    let mut out = String::new();
    use std::fmt::Write;

    writeln!(out, "  🌳 代理树").ok();
    writeln!(out, "  ├── 🧠 {} (main) [depth 0]", parent_name).ok();

    if subagents.is_empty() {
        writeln!(out, "  └── 无运行中的子代理").ok();
    } else {
        let indent = "  │   ";
        let last_idx = subagents.len() - 1;
        for (i, agent) in subagents.iter().enumerate() {
            let connector = if i == last_idx {
                "└── "
            } else {
                "├── "
            };
            writeln!(
                out,
                "{}{}🏗 {} [depth {}] — {} {}",
                indent,
                connector,
                agent.agent_type,
                agent.depth,
                agent.name,
                agent.status.label()
            )
            .ok();
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_tree_empty() {
        let result = render_subagent_tree_to_string(&[], "main");
        assert!(result.contains("🌳 代理树"));
        assert!(result.contains("无运行中的子代理"));
    }

    #[test]
    fn render_tree_with_subagents() {
        let subagents = vec![
            SubagentInfo {
                name: "架构设计".to_string(),
                depth: 1,
                agent_type: "Architect".to_string(),
                status: SubagentStatus::Done,
            },
            SubagentInfo {
                name: "代码实现".to_string(),
                depth: 1,
                agent_type: "Implementer".to_string(),
                status: SubagentStatus::Running,
            },
        ];
        let result = render_subagent_tree_to_string(&subagents, "main");
        assert!(result.contains("Architect"));
        assert!(result.contains("Implementer"));
        assert!(result.contains("✅"));
        assert!(result.contains("⏳"));
    }

    #[test]
    fn render_tree_single_subagent_no_indent() {
        let subagents = vec![SubagentInfo {
            name: "test".to_string(),
            depth: 1,
            agent_type: "Tester".to_string(),
            status: SubagentStatus::Pending,
        }];
        let result = render_subagent_tree_to_string(&subagents, "main");
        // Single subagent: should use └── connector
        assert!(result.contains("└──"));
    }
}

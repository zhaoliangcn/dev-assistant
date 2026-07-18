pub mod message_output;
pub use message_output::{CliMessageOutput, UIMessageOutput};

use std::io::{self, Write};
use unicode_width::UnicodeWidthStr;

// ── 前缀标签 ──────────────────────────────────────────────────────────

fn role_prefix(role: &str) -> &'static str {
    if role.starts_with("▸ 你") || role == "你"       { "👤 你" }
    else if role.starts_with("◂ 助手") || role == "助手" { "🤖 助手" }
    else if role.starts_with("⚙ 工具") || role == "工具" { "🔧 工具" }
    else if role.starts_with("▸ 成功") || role == "成功" { "✅ 成功" }
    else if role.starts_with("▸ 错误") || role == "错误" { "❌ 错误" }
    else if role.starts_with("▸ 警告") || role == "警告" { "⚠️ 警告" }
    else if role.starts_with("▸ 调试") || role == "调试" { "🐛 调试" }
    else if role.starts_with("▸ 信息") || role == "信息" { "ℹ️ 信息" }
    else                               { "📝 消息" }
}

/// 返回前缀字符串的显示宽度（用于多行缩进对齐）。
fn prefix_width(prefix: &str) -> usize {
    UnicodeWidthStr::width(prefix)
}

// ── 主渲染函数 ────────────────────────────────────────────────────────

/// Render the full UI with three panels:
///   - 输出面板 (conversation panel): message history
///   - 工具面板 (tool status panel): tool execution status
///   - 输入面板 (input panel): current input or status
///
/// `verbose` — when false, only show user messages and assistant responses
pub fn render(
    conversation: &[(String, String)],
    status: &[(String, String)],
    status_line: Option<&str>,
    verbose: bool,
) -> io::Result<()> {
    let mut stdout = io::stdout();
    let term_width = get_terminal_width().unwrap_or(80);

    // 清屏并将光标移到左上角
    print!("\x1b[2J\x1b[H");
    stdout.flush()?;

    // ── 标题栏 ──
    writeln!(stdout, "{}", "═".repeat(term_width))?;
    writeln!(stdout, "  Dev-Assistant — 消息窗口")?;
    writeln!(stdout, "{}", "═".repeat(term_width))?;

    // ── 工具面板（工具执行状态）──
    if !status.is_empty() {
        // 非 verbose 模式下只显示成功/错误/警告，隐藏信息/调试
        let status_visible: Vec<&(String, String)> = status.iter()
            .filter(|(role, _)| {
                if verbose { return true; }
                role == "成功" || role == "错误" || role == "警告"
            })
            .collect();

        if !status_visible.is_empty() {
            writeln!(stdout, "{}", "─".repeat(term_width))?;
            writeln!(stdout, "│ 工具面板")?;
            writeln!(stdout, "{}", "─".repeat(term_width))?;

            for (role, content) in status_visible {
                let prefix = role_prefix(role);
                let pw = prefix_width(prefix);
                for (i, line) in content.lines().enumerate() {
                    if line.is_empty() {
                        writeln!(stdout, "│")?;
                    } else if i == 0 {
                        writeln!(stdout, "│ {} │ {}", prefix, line)?;
                    } else {
                        // 后续行缩进对齐第一行的前缀位置
                        writeln!(stdout, "│ {:width$} │ {}", "", line, width = pw)?;
                    }
                }
                writeln!(stdout, "│")?;
            }
        }
    }

    // ── 输出面板（对话历史）──
    writeln!(stdout, "{}", "─".repeat(term_width))?;
    writeln!(stdout, "│ 输出面板")?;
    writeln!(stdout, "{}", "─".repeat(term_width))?;

    // 过滤消息：非 verbose 模式下只显示用户、助手对话和重要状态
    let visible: Vec<&(String, String)> = conversation.iter()
        .filter(|(role, _)| {
            if verbose { return true; }
            role.starts_with("▸ 你")
                || role.starts_with("◂ 助手")
                || role.starts_with("▸ 成功")
                || role.starts_with("▸ 错误")
                || role.starts_with("▸ 警告")
        })
        .collect();

    if visible.is_empty() {
        writeln!(stdout, "│ （等待消息...）")?;
    } else {
        for (role, content) in visible {
            let prefix = role_prefix(role);
            let pw = prefix_width(prefix);
            for (i, line) in content.lines().enumerate() {
                if line.is_empty() {
                    writeln!(stdout, "│")?;
                } else if i == 0 {
                    writeln!(stdout, "│ {} │ {}", prefix, line)?;
                } else {
                    // 后续行缩进对齐第一行的前缀位置
                    writeln!(stdout, "│ {:width$} │ {}", "", line, width = pw)?;
                }
            }
            writeln!(stdout, "│")?;
        }
    }

    // ── 分隔线 ──
    writeln!(stdout, "{}", "─".repeat(term_width))?;

    // ── 输入面板 ──
    match status_line {
        Some(status) => {
            writeln!(stdout, "│ 输入面板 — {}", status)?;
        }
        None => {
            writeln!(stdout, "│ 输入面板")?;
            write!(stdout, "│ > ")?;
        }
    }
    // 清除可能残留的旧内容
    write!(stdout, "\x1b[J")?;
    stdout.flush()?;

    Ok(())
}

/// Get terminal width, returns None if unavailable
fn get_terminal_width() -> Option<usize> {
    #[cfg(unix)]
    {
        use libc::ioctl;
        use libc::STDOUT_FILENO;
        use libc::TIOCGWINSZ;

        let mut winsize: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { ioctl(STDOUT_FILENO, TIOCGWINSZ, &mut winsize) } == 0 {
            return Some(winsize.ws_col as usize);
        }
    }

    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&w| w > 0)
}
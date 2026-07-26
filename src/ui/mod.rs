pub mod blocks;
pub mod markdown;
pub mod output_impls;
pub use blocks::MessageBlock;
pub use markdown::MarkdownRenderer;
pub use output_impls::{CliMessageOutput, UIMessageOutput};

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

// ── 新 API：块级渲染 ──────────────────────────────────────────────────

/// 渲染单个消息块（追加模式，不清屏）
/// 
/// 这是新的核心渲染 API，支持流式输出，保留终端滚动历史。
pub fn render_block(
    block: &MessageBlock,
    markdown_renderer: &MarkdownRenderer,
) -> io::Result<()> {
    let mut stdout = io::stdout();
    let term_width = get_terminal_width().unwrap_or(80);
    
    // 分隔线块
    if matches!(block, MessageBlock::Divider) {
        writeln!(stdout, "{}", "─".repeat(term_width))?;
        stdout.flush()?;
        return Ok(());
    }
    
    let prefix = block.prefix();
    let pw = prefix_width(prefix);
    
    // 获取内容（需要 Markdown 渲染的块先渲染）
    let content = if block.needs_markdown() {
        markdown_renderer.render(&block.content())
    } else {
        block.content()
    };
    
    // 打印消息分隔线
    writeln!(stdout)?;
    writeln!(stdout, "{}", "─".repeat(term_width))?;
    
    for (i, line) in content.lines().enumerate() {
        if line.is_empty() {
            writeln!(stdout)?;
        } else if i == 0 {
            writeln!(stdout, "{} │ {}", prefix, line)?;
        } else {
            writeln!(stdout, "{:width$} │ {}", "", line, width = pw)?;
        }
    }
    
    stdout.flush()?;
    Ok(())
}

/// 渲染消息块列表（追加模式）
pub fn render_blocks(
    blocks: &[MessageBlock],
    markdown_renderer: &MarkdownRenderer,
) -> io::Result<()> {
    for block in blocks {
        render_block(block, markdown_renderer)?;
    }
    Ok(())
}

/// 更新输入面板（清除旧内容，显示新状态）
pub fn render_input_panel(status_line: Option<&str>) -> io::Result<()> {
    let mut stdout = io::stdout();
    
    // 移动到行首并清除当前行
    write!(stdout, "\x1b[1G\x1b[K")?;
    
    match status_line {
        Some(status) => writeln!(stdout, "│ 输入面板 — {}", status)?,
        None => write!(stdout, "│ > ")?,
    }
    
    // 清除行尾残留内容
    write!(stdout, "\x1b[J")?;
    stdout.flush()?;
    
    Ok(())
}

/// 初始化 UI（显示标题栏）
pub fn init_ui() -> io::Result<()> {
    let mut stdout = io::stdout();
    let term_width = get_terminal_width().unwrap_or(80);
    
    writeln!(stdout, "{}", "═".repeat(term_width))?;
    writeln!(stdout, "  Dev-Assistant — 消息窗口")?;
    writeln!(stdout, "{}", "═".repeat(term_width))?;
    writeln!(stdout)?;
    
    stdout.flush()?;
    Ok(())
}

// ── 兼容旧 API：render() ──────────────────────────────────────────────

/// Render the full UI with three panels (兼容旧 API，使用追加模式)
/// 
/// 注意：此函数现在使用追加模式渲染，不再清除屏幕。
pub fn render(
    conversation: &[(String, String)],
    status: &[(String, String)],
    status_line: Option<&str>,
    verbose: bool,
) -> io::Result<()> {
    let mut stdout = io::stdout();
    let term_width = get_terminal_width().unwrap_or(80);

    // ── 工具面板（工具执行状态）──
    if !status.is_empty() {
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

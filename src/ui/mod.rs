pub mod blocks;
pub mod input;
pub mod markdown;
pub mod output_impls;
pub mod style;
pub use blocks::MessageBlock;
pub use markdown::MarkdownRenderer;
pub use output_impls::{CliMessageOutput, UIMessageOutput};

use std::io::{self, Write};
use unicode_width::UnicodeWidthStr;

// ── 新 API：块级渲染 ──────────────────────────────────────────────────

/// 返回前缀字符串的显示宽度（用于多行缩进对齐）。
fn prefix_width(prefix: &str) -> usize {
    UnicodeWidthStr::width(prefix)
}

/// 折叠后最多保留的行数（超出的显示截断提示）。
const COLLAPSED_MAX_LINES: usize = 20;

/// 全局存储最后一条被截断的完整内容（用于 `/expand` 命令）。
use std::sync::Mutex;
static LAST_TRUNCATED_CONTENT: Mutex<Option<String>> = Mutex::new(None);

/// 获取最后一条被截断的内容（由 `/expand` 调用）。
pub fn get_last_truncated_content() -> Option<String> {
    LAST_TRUNCATED_CONTENT.lock().ok().and_then(|mut guard| guard.take())
}

/// 将消息块渲染为字符串（纯渲染，无 IO 操作）。
///
/// 方便测试：传入 blocks 和终端宽度，返回 ANSI 格式化的字符串。
/// 测试中无需 mock stdout，直接断言输出内容即可。
pub fn render_blocks_to_string(
    blocks: &[MessageBlock],
    markdown_renderer: &MarkdownRenderer,
    term_width: usize,
) -> String {
    let mut buf = Vec::new();

    for block in blocks {
        // 分隔线块
        if matches!(block, MessageBlock::Divider) {
            let _ = writeln!(buf, "{}", "─".repeat(term_width));
            continue;
        }

        // Diff 块使用 full_prefix（含文件路径），其他块用 prefix
        let prefix = if matches!(block, MessageBlock::Diff { .. }) {
            block.full_prefix()
        } else {
            block.prefix().to_string()
        };
        let pw = prefix_width(&prefix);

        // 获取内容（需要 Markdown 渲染的块先渲染）
        let content = if block.needs_markdown() {
            markdown_renderer.render(&block.content())
        } else {
            block.content()
        };

        // 消息分隔线
        let _ = writeln!(buf);
        let _ = writeln!(buf, "{}", "─".repeat(term_width));

        // 对 ToolResult 块：折叠长内容，保留完整内容用于 /expand
        let rendering_content = if matches!(block, MessageBlock::ToolResult { .. }) {
            let lines: Vec<&str> = content.lines().collect();
            if lines.len() > COLLAPSED_MAX_LINES {
                let truncated: Vec<&str> = lines[..COLLAPSED_MAX_LINES].to_vec();
                let remaining = lines.len() - COLLAPSED_MAX_LINES;
                // 存储完整内容供 /expand
                if let Ok(mut guard) = LAST_TRUNCATED_CONTENT.lock() {
                    *guard = Some(content.clone());
                }
                let mut result = truncated.join("\n");
                result.push_str(&format!(
                    "\n\x1b[2m... 还有 {} 行（输入 /expand 查看完整内容）\x1b[0m",
                    remaining
                ));
                result
            } else {
                content
            }
        } else {
            content
        };

        for (i, line) in rendering_content.lines().enumerate() {
            if line.is_empty() {
                let _ = writeln!(buf);
            } else if i == 0 {
                let _ = writeln!(buf, "{} │ {}", prefix, line);
            } else {
                let _ = writeln!(buf, "{:width$} │ {}", "", line, width = pw);
            }
        }
    }

    String::from_utf8(buf).unwrap_or_default()
}

/// 渲染单个消息块（追加模式，不清屏）
/// 
/// 这是新的核心渲染 API，支持流式输出，保留终端滚动历史。
pub fn render_block(
    block: &MessageBlock,
    markdown_renderer: &MarkdownRenderer,
) -> io::Result<()> {
    let term_width = get_terminal_width().unwrap_or(80);
    let output = render_blocks_to_string(std::slice::from_ref(block), markdown_renderer, term_width);
    let mut stdout = io::stdout();
    write!(stdout, "{}", output)?;
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

// ── 进度条 ────────────────────────────────────────────────────────────

/// 渲染进度条到标准输出。
///
/// `current`: 当前进度（0-based）
/// `total`: 总进度
/// `label`: 当前阶段名称（如 "🏗 架构设计"）
/// `start_time`: 开始时间戳（用于计算耗时）
/// `status`: 当前状态描述
pub fn render_progress_bar(
    current: usize,
    total: usize,
    label: &str,
    start_time: &std::time::Instant,
    status: &str,
) -> io::Result<()> {
    let term_width = get_terminal_width().unwrap_or(80);
    // 进度条宽度
    let bar_width = (term_width.saturating_sub(20)).clamp(10, 60);
    let elapsed = start_time.elapsed();
    let progress = if total > 0 { current as f64 / total as f64 } else { 0.0 };
    let percent = (progress * 100.0) as usize;
    let filled = (progress * bar_width as f64) as usize;
    let empty = bar_width.saturating_sub(filled);

    let bar = format!(
        "\x1b[38;2;72;187;120m{}\x1b[0m{}",
        "█".repeat(filled),
        "░".repeat(empty),
    );

    let time_str = if elapsed.as_secs() > 60 {
        format!("{}m{:02}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
    } else {
        format!("{}s", elapsed.as_secs())
    };

    let mut stdout = io::stdout();
    // 清除当前行
    write!(stdout, "\x1b[2K\r")?;
    writeln!(
        stdout,
        "🚀 流水线 {} │ [{}] {:3}%  {:>10}",
        label, bar, percent, time_str
    )?;
    if !status.is_empty() {
        writeln!(stdout, "   {}", status)?;
    }
    stdout.flush()?;
    Ok(())
}

/// 更新输入提示行（清除旧内容，显示新状态）。
/// 无状态时仅显示 `> ` 提示符；有状态时显示 `⌛ 状态信息`。
pub fn render_input_panel(status_line: Option<&str>) -> io::Result<()> {
    let mut stdout = io::stdout();
    
    // 移动到行首并清除当前行
    write!(stdout, "\x1b[1G\x1b[K")?;
    
    match status_line {
        Some(status) => write!(stdout, "{}", status)?,
        None => write!(stdout, "> ")?,
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

pub mod blocks;
pub mod budget;
pub mod input;
pub mod markdown;
pub mod output_impls;
pub mod pipeline_view;
pub mod realtime_output;
pub mod status_bar;
pub mod style;
pub mod subagent_tree;
pub mod theme;
pub mod translucent;
pub use blocks::MessageBlock;
pub use budget::{render_budget_bar, render_budget_detail};
pub use markdown::MarkdownRenderer;
pub use output_impls::{CliMessageOutput, UIMessageOutput};
pub use pipeline_view::render_pipeline_progress;
pub use realtime_output::RealtimeOutput;
pub use status_bar::StatusBar;
pub use subagent_tree::{render_subagent_tree, SubagentInfo, SubagentStatus};

use std::io::{self, IsTerminal, Write};
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
///
/// 使用 `clone()` 而非 `take()`，支持多次调用 `/expand`。
/// 新截断发生时自动覆盖旧值（见 `render_blocks_to_string`）。
pub fn get_last_truncated_content() -> Option<String> {
    LAST_TRUNCATED_CONTENT
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
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
            let _ = writeln!(buf, "{}", "╌".repeat(term_width));
            continue;
        }

        // 使用 render_prefix() 统一前缀选择逻辑（Diff 含文件路径，其他用标准前缀）
        let prefix = block.render_prefix();
        let pw = prefix_width(&prefix);

        // 获取内容（需要 Markdown 渲染的块先渲染）
        let content = if block.needs_markdown() {
            markdown_renderer.render(&block.content())
        } else {
            block.content()
        };

        // 空内容块跳过渲染，避免输出孤立的 ╌ 分隔线
        if content.is_empty() {
            continue;
        }

        // 消息分隔线
        let _ = writeln!(buf);
        let _ = writeln!(buf, "{}", "╌".repeat(term_width));

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
                let theme = crate::ui::theme::active_theme();
                result.push_str(&format!(
                    "\n{}... 还有 {} 行（输入 /expand 查看完整内容）{}",
                    theme.muted_fg,
                    remaining,
                    crate::ui::theme::RESET
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
pub fn render_block(block: &MessageBlock, markdown_renderer: &MarkdownRenderer) -> io::Result<()> {
    let term_width = get_terminal_width().unwrap_or(80);
    let output =
        render_blocks_to_string(std::slice::from_ref(block), markdown_renderer, term_width);
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

/// 状态类型枚举，用于精确分类状态消息（避免字符串 contains 误匹配）。
///
/// 同时用于 `enhance_status`（UI 状态栏显示）和 `derive_thinking_status`
///（REPL 上下文提示），确保两处语义一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusType {
    ToolCall,
    WaitingLLM,
    Analyzing,
    Reading,
    Done,
    Other,
}

impl From<&str> for StatusType {
    fn from(s: &str) -> Self {
        // 优先按 emoji 前缀精确分类，避免文本误匹配；
        // 关键词回退时，具体词（分析/读取/搜索）优先于宽泛词（LLM/API），
        // 避免 "LLM 分析中" 之类消息被误分类为 WaitingLLM。
        if s.starts_with("🔧") {
            Self::ToolCall
        } else if s.starts_with("🔍") {
            Self::Analyzing
        } else if s.starts_with("📂") {
            Self::Reading
        } else if s.starts_with("🤖") {
            Self::WaitingLLM
        } else if s.starts_with("✅") {
            Self::Done
        } else if s.contains("工具调用") || s.contains("执行工具") {
            Self::ToolCall
        } else if s.contains("分析") || s.contains("检查") {
            Self::Analyzing
        } else if s.contains("读取文件")
            || s.contains("读取")
            || s.contains("搜索")
            || s.contains("search")
        {
            Self::Reading
        } else if s.contains("等待 LLM")
            || s.contains("LLM 正在思考")
            || s.contains("发送请求")
            || s.contains("LLM")
            || s.contains("API")
        {
            Self::WaitingLLM
        } else if s.contains("完成")
            || s.contains("处理完成")
            || s.contains("done")
            || s.contains("成功")
            || s.contains("success")
        {
            Self::Done
        } else {
            Self::Other
        }
    }
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
    // 每次渲染时重新检测终端宽度，响应窗口 resize
    let term_width = get_terminal_width().unwrap_or(80);
    // 进度条宽度
    let bar_width = (term_width.saturating_sub(20)).clamp(10, 60);
    let elapsed = start_time.elapsed();
    let progress = if total > 0 {
        current as f64 / total as f64
    } else {
        0.0
    };
    let percent = (progress * 100.0) as usize;
    let filled = (progress * bar_width as f64) as usize;
    let empty = bar_width.saturating_sub(filled);

    let bar = format!(
        "{}{}{}{}",
        crate::ui::theme::active_theme().success_fg,
        "█".repeat(filled),
        crate::ui::theme::RESET,
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
///
/// # 参数
/// - `status_line`: `Some(status)` 显示状态信息；`None` 时仅清除行，不写入任何内容。
///
/// # 设计说明
/// - 有状态时：清除旧行 → 写入增强后的状态文本 → 清除行尾残留
/// - 无状态时：仅清除整行，**不写入任何内容**。因为调用方（如 `read_line`）会自行处理 prompt，
///   若此处也写入 `> ` 会导致 prompt 重复或叠加。
pub fn render_input_panel(status_line: Option<&str>) -> io::Result<()> {
    let mut stdout = io::stdout();

    // 使用 \r 回到行首并清除整行，与流式输出策略一致
    write!(stdout, "\r\x1b[2K")?;

    if let Some(status) = status_line {
        // 增强状态栏：使用枚举精确分类，避免字符串 contains 误匹配
        let enhanced = enhance_status(status);
        write!(stdout, "{}", enhanced)?;
    }
    // 当 status_line 为 None 时，仅清除行，不写入任何内容
    // prompt 由调用方（如 rustyline 的 read_line）处理

    // 清除行尾残留内容
    write!(stdout, "\x1b[J")?;
    stdout.flush()?;

    Ok(())
}

/// 增强状态栏文本，根据状态类型添加更丰富的视觉样式。
///
/// 使用 [`StatusType`] 枚举进行分类，优先匹配 emoji 前缀，再回退到关键词。
fn enhance_status(status: &str) -> String {
    // 移除可能已有的 spinner 前缀
    let clean = status.trim_start_matches("⏳ ").trim_start_matches("⌛ ");

    match StatusType::from(clean) {
        StatusType::ToolCall => format!("🔧 {}", clean),
        StatusType::WaitingLLM => format!("🤖 {}", clean),
        StatusType::Analyzing => format!("🔍 {}", clean),
        StatusType::Reading => format!("📂 {}", clean),
        StatusType::Done => format!("✅ {}", clean),
        StatusType::Other => format!("⏳ {}", clean),
    }
}

/// 初始化 UI（显示标题栏 + 快捷键提示）
pub fn init_ui() -> io::Result<()> {
    let mut stdout = io::stdout();

    // 非终端（管道/重定向输出）时不打印标题栏，避免污染日志/文件输出
    if !stdout.is_terminal() {
        return Ok(());
    }

    let term_width = get_terminal_width().unwrap_or(80);

    writeln!(stdout, "{}", "═".repeat(term_width))?;
    writeln!(stdout, "  Dev-Assistant — 消息窗口")?;
    writeln!(stdout, "{}", "═".repeat(term_width))?;
    // T6: 快捷键提示行（灰色，启动时显示一次）
    let theme = crate::ui::theme::active_theme();
    writeln!(
        stdout,
        "{}  快捷键: Tab 补全 / 上下键 历史 / Ctrl+D 退出 / /help 查看命令{}",
        theme.muted_fg,
        crate::ui::theme::RESET
    )?;
    writeln!(stdout)?;

    stdout.flush()?;
    Ok(())
}

/// Get terminal width, returns None if unavailable.
///
/// 跨平台统一获取：Unix 使用 ioctl，其他平台回退到 COLUMNS 环境变量。
pub fn get_terminal_width() -> Option<usize> {
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

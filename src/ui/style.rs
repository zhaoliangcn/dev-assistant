//! UI 样式常量（颜色、间距、符号）
//!
//! # 约定
//! - 使用 `const` 而非 `static`，编译时内联，无运行时开销。
//! - 新代码应优先引用这些常量而非硬编码 ANSI 序列，以保持一致性。
//! - 未使用的常量会被保留（`#[allow(dead_code)]`），方便后续 UI 开发直接使用。

#![allow(dead_code)]

// ── ANSI 颜色 ──────────────────────────────────────────────────────────

/// 重置所有样式
pub const RESET: &str = "\x1b[0m";

/// 粗体
pub const BOLD: &str = "\x1b[1m";

/// 暗淡（灰色）
pub const DIM: &str = "\x1b[2m";

/// 斜体
pub const ITALIC: &str = "\x1b[3m";

/// 下划线
pub const UNDERLINE: &str = "\x1b[4m";

// ── 前景色 (24-bit) ────────────────────────────────────────────────────

/// 蓝色（行内代码用）
pub const CODE_BLUE: &str = "\x1b[38;2;156;189;248m";

/// 红色（错误用）
pub const ERROR_RED: &str = "\x1b[38;2;239;68;68m";

/// 绿色（成功用）
pub const SUCCESS_GREEN: &str = "\x1b[38;2;72;187;120m";

/// 黄色（标题用）
pub const HEADING_YELLOW: &str = "\x1b[1;33m";

/// 灰色（系统消息用）
pub const SYSTEM_GRAY: &str = "\x1b[2m";

/// 青色（工具调用用）
pub const TOOL_CYAN: &str = "\x1b[38;2;79;193;255m";

/// 链接蓝色
pub const LINK_BLUE: &str = "\x1b[4;34m";

// ── 分隔线字符 ─────────────────────────────────────────────────────────

/// 消息分隔线字符
pub const SEPARATOR: &str = "─";

/// 标题分隔线字符
pub const TITLE_SEPARATOR: &str = "═";

/// 竖线（内容缩进前缀）
pub const VERTICAL_BAR: &str = "│";

// ── 图标 ───────────────────────────────────────────────────────────────

/// 用户图标
pub const ICON_USER: &str = "👤";

/// 助手图标
pub const ICON_ASSISTANT: &str = "🤖";

/// 思考图标
pub const ICON_THINKING: &str = "💭";

/// 工具调用图标
pub const ICON_TOOL: &str = "🔧";

/// 成功图标
pub const ICON_SUCCESS: &str = "✅";

/// 失败图标
pub const ICON_ERROR: &str = "❌";

/// 警告图标
pub const ICON_WARNING: &str = "⚠️";

/// 信息图标
pub const ICON_INFO: &str = "ℹ️";

// ── 输入面板 ───────────────────────────────────────────────────────────

/// 输入提示符
pub const INPUT_PROMPT: &str = "│ > ";

/// 面板标题：输入面板
pub const INPUT_PANEL_LABEL: &str = "│ 输入面板";

/// 面板标题：工具面板
pub const TOOL_PANEL_LABEL: &str = "│ 工具面板";

/// 面板标题：输出面板
pub const OUTPUT_PANEL_LABEL: &str = "│ 输出面板";

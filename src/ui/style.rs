//! UI 样式常量（颜色、间距、符号）
//!
//! # 约定
//! - 颜色统一由 [`crate::ui::theme`] 管理（三层语义化调色板，支持亮/暗自适应）。
//!   本模块的历史颜色常量仅作为**向后兼容别名**转发到暗色主题值，
//!   新代码应使用 [`crate::ui::theme::active_theme`] 取角色色。
//! - 图标、分隔线等非颜色常量保留在本模块。
//! - 使用 `const` 而非 `static`，编译时内联，无运行时开销。

#![allow(dead_code)]

use crate::ui::theme::Theme;

// ── 颜色常量（向后兼容别名，转发自暗色主题） ─────────────────────────────

/// 重置所有样式
pub const RESET: &str = crate::ui::theme::RESET;

/// 粗体
pub const BOLD: &str = crate::ui::theme::BOLD;

/// 暗淡（灰色）
pub const DIM: &str = crate::ui::theme::DIM;

/// 斜体
pub const ITALIC: &str = crate::ui::theme::ITALIC;

/// 下划线
pub const UNDERLINE: &str = crate::ui::theme::UNDERLINE;

/// 蓝色（行内代码用）→ 语义角色 `code_fg`
pub const CODE_BLUE: &str = Theme::dark().code_fg;

/// 红色（错误用）→ 语义角色 `error_fg`
pub const ERROR_RED: &str = Theme::dark().error_fg;

/// 绿色（成功用）→ 语义角色 `success_fg`
pub const SUCCESS_GREEN: &str = Theme::dark().success_fg;

/// 黄色（标题用）→ 语义角色 `heading_fg`
pub const HEADING_YELLOW: &str = Theme::dark().heading_fg;

/// 灰色（系统消息用）→ 语义角色 `muted_fg`
pub const SYSTEM_GRAY: &str = Theme::dark().muted_fg;

/// 青色（工具调用用）→ 语义角色 `tool_fg`
pub const TOOL_CYAN: &str = Theme::dark().tool_fg;

/// 链接蓝色 → 语义角色 `link_fg`
pub const LINK_BLUE: &str = Theme::dark().link_fg;

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

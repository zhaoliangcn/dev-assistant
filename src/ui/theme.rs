//! 三层语义化调色板（参考 CodeWhale `crates/tui/src/palette` 架构）
//!
//! # 结构
//! 1. **第 1 层：RGB 元组**（`*_RGB` 常量）— 原始色值，带注释说明用途。
//! 2. **第 2 层：语义 ANSI 序列**（`Theme` 结构体字段）— 按 UI 角色命名
//!    （code/error/success/heading/muted/tool/link/warning/diff），
//!    而非"蓝色/绿色"，换主题时无需改调用点。
//! 3. **第 3 层：主题预设 + 自动检测** — [`Theme::dark`] / [`Theme::light`]
//!    两套完整预设；[`detect_mode`] 参考 CodeWhale `palette/detect.rs`：
//!    `COLORFGBG` 环境变量优先，macOS 回退读 `AppleInterfaceStyle`，默认暗色。
//!
//! 暗色主题色值与历史硬编码完全一致（可见输出不变）；亮色主题面向
//! `COLORFGBG >= 8` 的浅色终端，保证正文可读。

use std::sync::OnceLock;

// ═══════════════════════════════════════════════════════════════════════
// 第 1 层：RGB 元组（原始色值）
// ═══════════════════════════════════════════════════════════════════════

// ── 暗色系（对齐 CodeWhale Whale Dark 深海军蓝氛围） ──

/// 行内代码前景（暗色）：#9CBCF8 淡蓝
#[allow(dead_code)]
pub const DARK_CODE_FG_RGB: (u8, u8, u8) = (156, 189, 248);
/// 错误/失败前景（暗色）：#EF4444 红
#[allow(dead_code)]
pub const DARK_ERROR_FG_RGB: (u8, u8, u8) = (239, 68, 68);
/// 成功/新增前景（暗色）：#48BB78 绿
#[allow(dead_code)]
pub const DARK_SUCCESS_FG_RGB: (u8, u8, u8) = (72, 187, 120);
/// 工具调用/进度前景（暗色）：#4FC1FF 青
#[allow(dead_code)]
pub const DARK_TOOL_FG_RGB: (u8, u8, u8) = (79, 193, 255);
/// 警告前景（暗色）：#F0A030 琥珀
#[allow(dead_code)]
pub const DARK_WARNING_FG_RGB: (u8, u8, u8) = (240, 160, 48);
/// 输入提示符（暗色）：#808080 灰
#[allow(dead_code)]
pub const DARK_INPUT_PROMPT_FG_RGB: (u8, u8, u8) = (128, 128, 128);
/// diff 新增行背景（暗色）：#122A22 深绿（对齐 CodeWhale WHALE_DIFF_ADDED_BG）
#[allow(dead_code)]
pub const DARK_DIFF_ADDED_BG_RGB: (u8, u8, u8) = (18, 42, 34);
/// diff 删除行背景（暗色）：#2A121A 深红（对齐 CodeWhale WHALE_DIFF_DELETED_BG）
#[allow(dead_code)]
pub const DARK_DIFF_DELETED_BG_RGB: (u8, u8, u8) = (42, 18, 26);

// ── 亮色系（浅色终端高对比） ──

/// 行内代码前景（亮色）：#1E50A0 深蓝
#[allow(dead_code)]
pub const LIGHT_CODE_FG_RGB: (u8, u8, u8) = (30, 80, 160);
/// 错误/失败前景（亮色）：#B91C1C 深红
#[allow(dead_code)]
pub const LIGHT_ERROR_FG_RGB: (u8, u8, u8) = (185, 28, 28);
/// 成功/新增前景（亮色）：#15803D 深绿
#[allow(dead_code)]
pub const LIGHT_SUCCESS_FG_RGB: (u8, u8, u8) = (21, 128, 61);
/// 工具调用/进度前景（亮色）：#0066CC 蓝
#[allow(dead_code)]
pub const LIGHT_TOOL_FG_RGB: (u8, u8, u8) = (0, 102, 204);
/// 警告前景（亮色）：#92400E 深琥珀
#[allow(dead_code)]
pub const LIGHT_WARNING_FG_RGB: (u8, u8, u8) = (146, 64, 14);
/// 标题前景（亮色）：#92400E 深琥珀（粗体）
#[allow(dead_code)]
pub const LIGHT_HEADING_FG_RGB: (u8, u8, u8) = (146, 64, 14);
/// 链接前景（亮色）：#1D4ED8 蓝
#[allow(dead_code)]
pub const LIGHT_LINK_FG_RGB: (u8, u8, u8) = (29, 78, 216);
/// 输入提示符（亮色）：#64748B 灰蓝
#[allow(dead_code)]
pub const LIGHT_INPUT_PROMPT_FG_RGB: (u8, u8, u8) = (100, 116, 139);
/// diff 新增行背景（亮色）：#D8F0DA 浅绿
#[allow(dead_code)]
pub const LIGHT_DIFF_ADDED_BG_RGB: (u8, u8, u8) = (216, 240, 218);
/// diff 删除行背景（亮色）：#F8D6D6 浅红
#[allow(dead_code)]
pub const LIGHT_DIFF_DELETED_BG_RGB: (u8, u8, u8) = (248, 214, 214);

// ═══════════════════════════════════════════════════════════════════════
// 修饰符（与主题无关）
// ═══════════════════════════════════════════════════════════════════════

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

/// 生成 24-bit 前景色 ANSI 序列的辅助函数（仅文档用途）。
///
/// 实际语义常量在 [`Theme::dark`] / [`Theme::light`] 中手写为字符串字面量
/// （`const fn` 无法动态拼接字符串），因此不提供此函数。
#[allow(dead_code)]
const _: fn(u8, u8, u8) -> &'static str = |r, g, b| {
    let _ = (r, g, b);
    "\x1b[38;2;0;0;0m"
};

// ═══════════════════════════════════════════════════════════════════════
// 第 2+3 层：主题预设（语义 ANSI 序列）
// ═══════════════════════════════════════════════════════════════════════

/// 终端主题模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

/// 语义化主题：每个字段是 UI 角色的 24-bit 前景 ANSI 序列。
///
/// 字段名按角色命名（code/error/success/…），调用点只引用角色，
/// 切换主题（dark/light）时无需改动渲染代码。
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub mode: ThemeMode,

    /// 行内代码前景
    pub code_fg: &'static str,
    /// 错误/失败前景
    pub error_fg: &'static str,
    /// 成功/新增前景
    pub success_fg: &'static str,
    /// 标题前景（粗体）
    pub heading_fg: &'static str,
    /// 暗淡/次要文本（等价 `DIM`）
    pub muted_fg: &'static str,
    /// 工具调用/进度前景
    pub tool_fg: &'static str,
    /// 链接前景
    pub link_fg: &'static str,
    /// 警告前景
    #[allow(dead_code)]
    pub warning_fg: &'static str,
    /// 输入提示符前景
    pub input_prompt_fg: &'static str,

    // Diff 配色（与 markdown.rs 历史值一致，保持可见输出不变）
    /// diff 新增行（绿）
    pub diff_added_fg: &'static str,
    /// diff 删除行（红）
    pub diff_deleted_fg: &'static str,
    /// diff hunk 头（青色粗体）
    pub diff_hunk_fg: &'static str,
    /// diff 新增行背景（绿底，配合 `diff_added_fg`）
    pub diff_added_bg: &'static str,
    /// diff 删除行背景（红底，配合 `diff_deleted_fg`）
    pub diff_deleted_bg: &'static str,
}

impl Theme {
    /// 暗色主题 — 色值与历史硬编码完全一致，保证现有输出与测试不变。
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            code_fg: "\x1b[38;2;156;189;248m",
            error_fg: "\x1b[38;2;239;68;68m",
            success_fg: "\x1b[38;2;72;187;120m",
            heading_fg: "\x1b[1;33m",
            muted_fg: "\x1b[2m",
            tool_fg: "\x1b[38;2;79;193;255m",
            link_fg: "\x1b[4;34m",
            warning_fg: "\x1b[38;2;240;160;48m",
            input_prompt_fg: "\x1b[38;2;128;128;128m",
            diff_added_fg: "\x1b[38;2;72;187;120m",
            diff_deleted_fg: "\x1b[38;2;239;68;68m",
            diff_hunk_fg: "\x1b[1;38;2;79;193;255m",
            diff_added_bg: "\x1b[48;2;18;42;34m",
            diff_deleted_bg: "\x1b[48;2;42;18;26m",
        }
    }

    /// 亮色主题 — 面向浅色终端的高对比配色。
    #[must_use]
    pub const fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            code_fg: "\x1b[38;2;30;80;160m",
            error_fg: "\x1b[38;2;185;28;28m",
            success_fg: "\x1b[38;2;21;128;61m",
            heading_fg: "\x1b[1;38;2;146;64;14m",
            muted_fg: "\x1b[2m",
            tool_fg: "\x1b[38;2;0;102;204m",
            link_fg: "\x1b[4;38;2;29;78;216m",
            warning_fg: "\x1b[38;2;146;64;14m",
            input_prompt_fg: "\x1b[38;2;100;116;139m",
            diff_added_fg: "\x1b[38;2;21;128;61m",
            diff_deleted_fg: "\x1b[38;2;185;28;28m",
            diff_hunk_fg: "\x1b[1;38;2;0;102;204m",
            diff_added_bg: "\x1b[48;2;216;240;218m",
            diff_deleted_bg: "\x1b[48;2;248;214;214m",
        }
    }

    /// 按模式取对应预设。
    #[must_use]
    pub const fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        }
    }
}

/// diff 行背景的 RGB 色调（用于半透明混色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffTint {
    /// 新增行色调（绿）
    pub added: (u8, u8, u8),
    /// 删除行色调（红）
    pub deleted: (u8, u8, u8),
}

/// 按主题模式返回 diff 背景的 RGB 色调，供半透明混色使用。
#[must_use]
pub const fn diff_tint_rgb(mode: ThemeMode) -> DiffTint {
    match mode {
        ThemeMode::Dark => DiffTint {
            added: DARK_DIFF_ADDED_BG_RGB,
            deleted: DARK_DIFF_DELETED_BG_RGB,
        },
        ThemeMode::Light => DiffTint {
            added: LIGHT_DIFF_ADDED_BG_RGB,
            deleted: LIGHT_DIFF_DELETED_BG_RGB,
        },
    }
}

/// 解析 `COLORFGBG`（参考 CodeWhale `palette/detect.rs::from_colorfgbg`）。
///
/// 最后一个数字段是终端背景色，`>= 8` 视为亮色 profile。
#[must_use]
pub fn from_colorfgbg(value: &str) -> Option<ThemeMode> {
    let bg = value
        .split(';')
        .rev()
        .find_map(|part| part.parse::<u16>().ok())?;
    Some(if bg >= 8 { ThemeMode::Light } else { ThemeMode::Dark })
}

#[cfg(target_os = "macos")]
fn detect_macos_mode() -> Option<ThemeMode> {
    let output = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .ok()?;

    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout);
        Some(if value.trim().eq_ignore_ascii_case("dark") {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        })
    } else {
        Some(ThemeMode::Light)
    }
}

#[cfg(not(target_os = "macos"))]
fn detect_macos_mode() -> Option<ThemeMode> {
    None
}

/// 检测当前终端主题模式。
///
/// `COLORFGBG` 环境变量优先；macOS 外观是回退；缺失/不可解析默认暗色，
/// 保证既有终端环境保持原有观感。
#[must_use]
pub fn detect_mode() -> ThemeMode {
    std::env::var("COLORFGBG")
        .ok()
        .as_deref()
        .and_then(from_colorfgbg)
        .or_else(detect_macos_mode)
        .unwrap_or(ThemeMode::Dark)
}

static ACTIVE_THEME: OnceLock<Theme> = OnceLock::new();

/// 当前生效的主题（按 [`detect_mode`] 自动选择，进程内缓存）。
#[must_use]
pub fn active_theme() -> &'static Theme {
    ACTIVE_THEME.get_or_init(|| Theme::for_mode(detect_mode()))
}

#[cfg(test)]
mod tests {
    use super::{Theme, ThemeMode, active_theme, detect_mode, from_colorfgbg};

    #[test]
    fn from_colorfgbg_parses_background() {
        assert_eq!(from_colorfgbg("15;0"), Some(ThemeMode::Dark));
        assert_eq!(from_colorfgbg("15;15"), Some(ThemeMode::Light));
        assert_eq!(from_colorfgbg("10;8"), Some(ThemeMode::Light));
        assert_eq!(from_colorfgbg("garbage"), None);
    }

    #[test]
    fn dark_theme_matches_legacy_values() {
        let t = Theme::dark();
        assert_eq!(t.code_fg, "\x1b[38;2;156;189;248m");
        assert_eq!(t.error_fg, "\x1b[38;2;239;68;68m");
        assert_eq!(t.success_fg, "\x1b[38;2;72;187;120m");
        assert_eq!(t.tool_fg, "\x1b[38;2;79;193;255m");
        assert_eq!(t.diff_hunk_fg, "\x1b[1;38;2;79;193;255m");
    }

    #[test]
    fn light_theme_is_distinct() {
        let dark = Theme::dark();
        let light = Theme::light();
        assert_eq!(light.mode, ThemeMode::Light);
        assert_ne!(light.code_fg, dark.code_fg);
        assert_ne!(light.error_fg, dark.error_fg);
        assert_ne!(light.success_fg, dark.success_fg);
    }

    #[test]
    fn for_mode_matches() {
        assert_eq!(Theme::for_mode(ThemeMode::Dark).mode, ThemeMode::Dark);
        assert_eq!(Theme::for_mode(ThemeMode::Light).mode, ThemeMode::Light);
    }

    #[test]
    fn active_theme_is_consistent() {
        // 两次调用返回同一缓存实例
        let a = active_theme() as *const Theme;
        let b = active_theme() as *const Theme;
        assert_eq!(a, b);
    }

    #[test]
    fn detect_mode_never_panics() {
        let _ = detect_mode();
    }
}

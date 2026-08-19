//! 上下文预算小部件：将 [`ContextBudget`](crate::agent::context::ContextBudget) 渲染为终端 UI。
//!
//! # 渲染模式
//!
//! - **紧凑条**（[`render_budget_bar`]）：单行进度条 + 状态色，用于每轮工具调用后的常规展示。
//! - **详细面板**（[`render_budget_detail`]）：带边框的完整分解表，用于 `/budget` slash 命令。
//!
//! # 颜色编码
//!
//! | 压力等级 | 进度条颜色 | 状态标签 |
//! |---------|-----------|---------|
//! | Normal   | 绿色 (`success_fg`) | ✅ Normal |
//! | Warning  | 琥珀 (`warning_fg`) | ⚠️ Warning |
//! | Critical | 红色 (`error_fg`) | 🔴 Critical |
//! | Exhausted | 红色粗体 | 🚨 Exhausted |

use std::io::{self, IsTerminal, Write};

use unicode_width::UnicodeWidthStr;

use crate::agent::context::ContextBudget;
use crate::ui::theme::active_theme;
use crate::ui::translucent::{blend, detected_bg, enabled};

// ── 布局常量 ─────────────────────────────────────────────────────────

/// 紧凑进度条占用的列数（不含标签文本）。
const BAR_WIDTH_COMPACT: usize = 30;

/// 详细面板进度条宽度（自适应终端宽度，上限 60）。
const BAR_WIDTH_DETAIL_MAX: usize = 60;

/// Token 数量单位转换（K = 1000，仅用于显示）。
const K: usize = 1000;

// ---------------------------------------------------------------------------
// 工具函数
// ---------------------------------------------------------------------------

/// 将 token 数格式化为带单位的紧凑字符串：`12K`、`2.3K`、`262K`。
fn format_tokens(t: usize) -> String {
    if t >= K {
        let k = t as f64 / K as f64;
        if k >= 10.0 {
            format!("{:.0}K", k)
        } else {
            format!("{:.1}K", k)
        }
    } else {
        format!("{}B", t)
    }
}

/// 根据压力等级选择进度条颜色 ANSI 序列。
fn bar_color_for_pressure(budget: &ContextBudget) -> &'static str {
    let theme = active_theme();
    match budget.pressure {
        crate::agent::context::ContextPressure::Normal => theme.success_fg,
        crate::agent::context::ContextPressure::Warning => theme.warning_fg,
        crate::agent::context::ContextPressure::Critical => theme.error_fg,
        crate::agent::context::ContextPressure::Exhausted => theme.error_fg,
    }
}

/// 根据压力等级选择状态标签文本。
fn status_label(budget: &ContextBudget) -> &'static str {
    match budget.pressure {
        crate::agent::context::ContextPressure::Normal => "✅ Normal",
        crate::agent::context::ContextPressure::Warning => "⚠️ Warning",
        crate::agent::context::ContextPressure::Critical => "🔴 Critical",
        crate::agent::context::ContextPressure::Exhausted => "🚨 Exhausted",
    }
}

/// 根据压力等级选择状态标签颜色。
fn status_color(budget: &ContextBudget) -> &'static str {
    let theme = active_theme();
    match budget.pressure {
        crate::agent::context::ContextPressure::Normal => theme.success_fg,
        crate::agent::context::ContextPressure::Warning => theme.warning_fg,
        crate::agent::context::ContextPressure::Critical
        | crate::agent::context::ContextPressure::Exhausted => theme.error_fg,
    }
}

/// 构建进度条字符串：`████████████░░░░░░░░`。
///
/// `bar_width` 为总宽度，`utilization` 为使用率（0.0–1.0，超出则钳制到 [0, 1]）。
fn build_bar(bar_width: usize, utilization: f64) -> String {
    let filled = (utilization.clamp(0.0, 1.0) * bar_width as f64).round() as usize;
    let empty = bar_width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

/// 计算剩余可用 token 百分比字符串。
fn utilization_pct(budget: &ContextBudget) -> String {
    format!("{:.1}%", budget.utilization * 100.0)
}

// ---------------------------------------------------------------------------
// 紧凑进度条
// ---------------------------------------------------------------------------

/// 渲染紧凑上下文预算条（单行），追加到标准输出。
///
/// 格式：
/// ```
/// │  💬 12.4K / 262K (4.7%)  ████████░░░░░░░░░░░░░░  ✅ Normal
/// ```
///
/// 终端非 TTY（管道/文件重定向）时静默跳过，避免向非终端输出 ANSI 控制序列。
pub fn render_budget_bar(budget: &ContextBudget) -> io::Result<()> {
    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        return Ok(());
    }

    let bar_color = bar_color_for_pressure(budget);
    let bar = build_bar(BAR_WIDTH_COMPACT, budget.utilization);
    let pct = utilization_pct(budget);

    let mut line = String::from("│  💬 ");
    line.push_str(&format!(
        "{} / {}",
        format_tokens(budget.total_tokens),
        format_tokens(budget.max_tokens)
    ));
    line.push_str(" (");
    line.push_str(&pct);
    line.push(')');

    // 计算进度条前的标签宽度，动态填充空格使进度条对齐
    let prefix_width = UnicodeWidthStr::width(line.as_str());
    let total_available = get_terminal_width();
    // 留出：prefix + bar + 状态标签 + 前后空格
    let status_str = status_label(budget);
    let status_color = status_color(budget);
    let status_width = UnicodeWidthStr::width(status_str)
        + UnicodeWidthStr::width(status_color)
        + UnicodeWidthStr::width(crate::ui::theme::RESET)
        + 3; // 3 = "  " + "  "
    let bar_slot = total_available
        .saturating_sub(prefix_width)
        .saturating_sub(status_width);
    let bar_width = bar_slot.min(BAR_WIDTH_COMPACT).max(10);
    let bar = build_bar(bar_width, budget.utilization);

    let padding = bar_slot.saturating_sub(bar_width + 1);
    line.push(' ');
    line.push_str(&bar_color);
    line.push_str(&bar);
    line.push_str(crate::ui::theme::RESET);
    for _ in 0..padding {
        line.push(' ');
    }
    line.push_str(" ");
    line.push_str(status_color);
    line.push_str(status_str);
    line.push_str(crate::ui::theme::RESET);
    line.push_str("  │");

    writeln!(stdout, "\r",)?;
    writeln!(stdout, "{}", line)?;
    stdout.flush()?;
    Ok(())
}

/// 终端宽度检测。
fn get_terminal_width() -> usize {
    crate::ui::get_terminal_width().unwrap_or(80)
}

// ---------------------------------------------------------------------------
// 详细面板
// ---------------------------------------------------------------------------

/// 渲染详细上下文预算面板，追加到标准输出。
///
/// 格式：
/// ```
/// ╔══════════════════════════════════════════════════════════╗
/// ║  📊 上下文预算 (Context Budget)                         ║
/// ╠══════════════════════════════════════════════════════════╣
/// ║  System:    2.1K tokens (0.8%)                          ║
/// ║  Memory:    0.4K tokens (0.2%)                          ║
/// ║  History:  12.4K tokens (4.7%)                          ║
/// ║  Tools:     3.2K tokens (1.2%)                          ║
/// ╠══════════════════════════════════════════════════════════╣
/// ║  Total:    18.1K / 262K tokens (6.9%)                   ║
/// ║  ████████████░░░░░░░░░░░░░░░░░░  6.9%                    ║
/// ║  Status:   ✅ Normal                                    ║
/// ║  剩余:     243.9K tokens                                ║
/// ╚══════════════════════════════════════════════════════════╝
/// ```
pub fn render_budget_detail(budget: &ContextBudget) -> io::Result<()> {
    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        // 非终端时输出纯文本版本
        writeln!(stdout, "\n── Context Budget ──")?;
        writeln!(
            stdout,
            "  System:    {} tokens ({:.2}%)",
            format_tokens(budget.system_prompt_tokens),
            budget.system_prompt_tokens as f64 / budget.max_tokens as f64 * 100.0
        )?;
        writeln!(
            stdout,
            "  Memory:    {} tokens ({:.2}%)",
            format_tokens(budget.memory_tokens),
            budget.memory_tokens as f64 / budget.max_tokens as f64 * 100.0
        )?;
        writeln!(
            stdout,
            "  History:  {} tokens ({:.2}%)",
            format_tokens(budget.history_tokens),
            budget.history_tokens as f64 / budget.max_tokens as f64 * 100.0
        )?;
        writeln!(
            stdout,
            "  Tools:     {} tokens ({:.2}%)",
            format_tokens(budget.tool_schema_tokens),
            budget.tool_schema_tokens as f64 / budget.max_tokens as f64 * 100.0
        )?;
        writeln!(
            stdout,
            "  Total:    {} / {} tokens ({})",
            format_tokens(budget.total_tokens),
            format_tokens(budget.max_tokens),
            utilization_pct(budget)
        )?;
        writeln!(stdout, "  Status:   {}", status_label(budget))?;
        writeln!(
            stdout,
            "  Remaining: {} tokens",
            format_tokens(budget.estimated_room)
        )?;
        stdout.flush()?;
        return Ok(());
    }

    let term_width = get_terminal_width().min(BAR_WIDTH_DETAIL_MAX);
    let panel_width = term_width.min(66).max(44);
    let sep = "═".repeat(panel_width);
    let bar_color = bar_color_for_pressure(budget);
    let status_color = status_color(budget);
    let status_str = status_label(budget);

    let system_pct = budget.system_prompt_tokens as f64 / budget.max_tokens as f64 * 100.0;
    let memory_pct = budget.memory_tokens as f64 / budget.max_tokens as f64 * 100.0;
    let history_pct = budget.history_tokens as f64 / budget.max_tokens as f64 * 100.0;
    let tools_pct = budget.tool_schema_tokens as f64 / budget.max_tokens as f64 * 100.0;

    writeln!(stdout, "\r")?;
    writeln!(stdout, "╔{}╗", sep)?;
    writeln!(
        stdout,
        "║  📊 上下文预算 (Context Budget){:width$}║",
        "",
        width = panel_width.saturating_sub(25)
    )?;
    writeln!(stdout, "╠{}╣", sep)?;
    writeln!(
        stdout,
        "║  System:    {} tokens ({:.2}%){:width$}║",
        format_tokens(budget.system_prompt_tokens),
        system_pct,
        "",
        width = panel_width.saturating_sub(40)
    )?;
    writeln!(
        stdout,
        "║  Memory:    {} tokens ({:.2}%){:width$}║",
        format_tokens(budget.memory_tokens),
        memory_pct,
        "",
        width = panel_width.saturating_sub(40)
    )?;
    writeln!(
        stdout,
        "║  History:  {} tokens ({:.2}%){:width$}║",
        format_tokens(budget.history_tokens),
        history_pct,
        "",
        width = panel_width.saturating_sub(40)
    )?;
    writeln!(
        stdout,
        "║  Tools:     {} tokens ({:.2}%){:width$}║",
        format_tokens(budget.tool_schema_tokens),
        tools_pct,
        "",
        width = panel_width.saturating_sub(40)
    )?;
    writeln!(stdout, "╠{}╣", sep)?;

    let bar = build_bar(panel_width.saturating_sub(20), budget.utilization);
    let pct = utilization_pct(budget);

    writeln!(
        stdout,
        "║  Total:    {} / {} tokens ({}){:width$}║",
        format_tokens(budget.total_tokens),
        format_tokens(budget.max_tokens),
        pct,
        "",
        width = panel_width.saturating_sub(40)
    )?;
    writeln!(
        stdout,
        "║  {}{}{}  {:>6}{:width$}║",
        bar_color,
        bar,
        crate::ui::theme::RESET,
        pct,
        "",
        width = panel_width.saturating_sub(22)
    )?;
    writeln!(
        stdout,
        "║  Status:   {}{}{}{:width$}║",
        status_color,
        status_str,
        crate::ui::theme::RESET,
        "",
        width = panel_width.saturating_sub(12)
    )?;
    writeln!(
        stdout,
        "║  剩余:     {} tokens{:width$}║",
        format_tokens(budget.estimated_room),
        "",
        width = panel_width.saturating_sub(20)
    )?;
    writeln!(stdout, "╚{}╝", sep)?;

    stdout.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 纯渲染（无 IO，方便测试）
// ---------------------------------------------------------------------------

/// 渲染紧凑进度条为字符串（纯渲染，无 IO）。
pub fn render_budget_bar_to_string(budget: &ContextBudget, term_width: usize) -> String {
    let bar_color = bar_color_for_pressure(budget);
    let pct = utilization_pct(budget);

    let mut line = String::from("│  💬 ");
    line.push_str(&format!(
        "{} / {}",
        format_tokens(budget.total_tokens),
        format_tokens(budget.max_tokens)
    ));
    line.push_str(" (");
    line.push_str(&pct);
    line.push(')');

    let prefix_width = UnicodeWidthStr::width(line.as_str());
    let status_str = status_label(budget);
    let status_color = status_color(budget);
    let status_width = UnicodeWidthStr::width(status_str) + 4; // 空格 + 前缀/后缀
    let bar_slot = term_width
        .saturating_sub(prefix_width)
        .saturating_sub(status_width);
    let bar_width = bar_slot.min(BAR_WIDTH_COMPACT).max(10);
    let bar = build_bar(bar_width, budget.utilization);
    let padding = bar_slot.saturating_sub(bar_width + 1);

    line.push(' ');
    line.push_str(bar_color);
    line.push_str(&bar);
    line.push_str(crate::ui::theme::RESET);
    for _ in 0..padding {
        line.push(' ');
    }
    line.push_str(" ");
    line.push_str(status_color);
    line.push_str(status_str);
    line.push_str(crate::ui::theme::RESET);
    line.push_str("  │");

    line
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context::ContextPressure;

    fn make_budget(total: usize, max: usize, pressure: ContextPressure) -> ContextBudget {
        ContextBudget {
            system_prompt_tokens: 2100,
            memory_tokens: 400,
            history_tokens: total - 2100 - 400,
            tool_schema_tokens: 3200,
            total_tokens: total,
            max_tokens: max,
            utilization: total as f64 / max as f64,
            estimated_room: max.saturating_sub(total),
            pressure,
        }
    }

    #[test]
    fn format_tokens_rounds_correctly() {
        assert_eq!(format_tokens(12400), "12K");
        assert_eq!(format_tokens(2340), "2.3K");
        assert_eq!(format_tokens(900), "900B");
        assert_eq!(format_tokens(262144), "262K");
        assert_eq!(format_tokens(100000), "100K");
    }

    #[test]
    fn build_bar_fills_correctly() {
        let bar = build_bar(20, 0.0);
        assert_eq!(bar.chars().count(), 20);
        assert!(bar.chars().all(|c| c == '░'));

        let bar = build_bar(20, 1.0);
        assert_eq!(bar.chars().count(), 20);
        assert!(bar.chars().all(|c| c == '█'));

        let bar = build_bar(20, 0.5);
        let filled = bar.chars().filter(|c| *c == '█').count();
        let empty = bar.chars().filter(|c| *c == '░').count();
        assert_eq!(filled + empty, 20);
        assert!(filled >= 9 && filled <= 11); // 0.5 * 20 = 10
    }

    #[test]
    fn budget_bar_normal_status() {
        let budget = make_budget(12400, 262144, ContextPressure::Normal);
        let result = render_budget_bar_to_string(&budget, 80);
        assert!(result.contains("💬"));
        assert!(result.contains("12K"));
        assert!(result.contains("262K"));
        assert!(result.contains("✅ Normal"));
    }

    #[test]
    fn budget_bar_warning_status() {
        let budget = make_budget(157286, 262144, ContextPressure::Warning); // ~60%
        let result = render_budget_bar_to_string(&budget, 80);
        assert!(result.contains("⚠️ Warning"));
    }

    #[test]
    fn budget_bar_critical_status() {
        let budget = make_budget(217702, 262144, ContextPressure::Critical); // ~83%
        let result = render_budget_bar_to_string(&budget, 80);
        assert!(result.contains("🔴 Critical"));
    }

    #[test]
    fn budget_bar_exhausted_status() {
        let budget = make_budget(249036, 262144, ContextPressure::Exhausted); // ~95%
        let result = render_budget_bar_to_string(&budget, 80);
        assert!(result.contains("🚨 Exhausted"));
    }

    #[test]
    fn budget_detail_to_string_contains_all_fields() {
        let budget = make_budget(18100, 262144, ContextPressure::Normal);
        let result = render_budget_detail_to_string(&budget, 60);
        assert!(result.contains("System"));
        assert!(result.contains("Memory"));
        assert!(result.contains("History"));
        assert!(result.contains("Tools"));
        assert!(result.contains("Total"));
        assert!(result.contains("Status"));
        assert!(result.contains("剩余"));
        assert!(result.contains("262K"));
    }

    /// 渲染详细面板为字符串（纯渲染，无 IO）。
    fn render_budget_detail_to_string(budget: &ContextBudget, _term_width: usize) -> String {
        let panel_width = 44;
        let sep = "═".repeat(panel_width);
        let bar_color = bar_color_for_pressure(budget);
        let status_color = status_color(budget);
        let status_str = status_label(budget);

        let system_pct = budget.system_prompt_tokens as f64 / budget.max_tokens as f64 * 100.0;
        let memory_pct = budget.memory_tokens as f64 / budget.max_tokens as f64 * 100.0;
        let history_pct = budget.history_tokens as f64 / budget.max_tokens as f64 * 100.0;
        let tools_pct = budget.tool_schema_tokens as f64 / budget.max_tokens as f64 * 100.0;

        let mut out = String::new();
        use std::fmt::Write;

        writeln!(out, "╔{}╗", sep).ok();
        writeln!(
            out,
            "║  📊 上下文预算 (Context Budget){:width$}║",
            "",
            width = panel_width.saturating_sub(25)
        )
        .ok();
        writeln!(out, "╠{}╣", sep).ok();
        writeln!(
            out,
            "║  System:    {} tokens ({:.2}%){:width$}║",
            format_tokens(budget.system_prompt_tokens),
            system_pct,
            "",
            width = panel_width.saturating_sub(40)
        )
        .ok();
        writeln!(
            out,
            "║  Memory:    {} tokens ({:.2}%){:width$}║",
            format_tokens(budget.memory_tokens),
            memory_pct,
            "",
            width = panel_width.saturating_sub(40)
        )
        .ok();
        writeln!(
            out,
            "║  History:  {} tokens ({:.2}%){:width$}║",
            format_tokens(budget.history_tokens),
            history_pct,
            "",
            width = panel_width.saturating_sub(40)
        )
        .ok();
        writeln!(
            out,
            "║  Tools:     {} tokens ({:.2}%){:width$}║",
            format_tokens(budget.tool_schema_tokens),
            tools_pct,
            "",
            width = panel_width.saturating_sub(40)
        )
        .ok();
        writeln!(out, "╠{}╣", sep).ok();

        let bar = build_bar(panel_width.saturating_sub(20), budget.utilization);
        let pct = utilization_pct(budget);
        writeln!(
            out,
            "║  Total:    {} / {} tokens ({}){:width$}║",
            format_tokens(budget.total_tokens),
            format_tokens(budget.max_tokens),
            pct,
            "",
            width = panel_width.saturating_sub(40)
        )
        .ok();
        writeln!(
            out,
            "║  {}{}{}  {:>6}{:width$}║",
            bar_color,
            bar,
            crate::ui::theme::RESET,
            pct,
            "",
            width = panel_width.saturating_sub(22)
        )
        .ok();
        writeln!(
            out,
            "║  Status:   {}{}{}{:width$}║",
            status_color,
            status_str,
            crate::ui::theme::RESET,
            "",
            width = panel_width.saturating_sub(12)
        )
        .ok();
        writeln!(
            out,
            "║  剩余:     {} tokens{:width$}║",
            format_tokens(budget.estimated_room),
            "",
            width = panel_width.saturating_sub(20)
        )
        .ok();
        writeln!(out, "╚{}╝", sep).ok();

        out
    }

    #[test]
    fn bar_color_selects_success_for_normal() {
        let theme = active_theme();
        let budget = make_budget(10000, 262144, ContextPressure::Normal);
        assert_eq!(bar_color_for_pressure(&budget), theme.success_fg);
    }

    #[test]
    fn bar_color_selects_error_for_critical() {
        let theme = active_theme();
        let budget = make_budget(217702, 262144, ContextPressure::Critical);
        assert_eq!(bar_color_for_pressure(&budget), theme.error_fg);
    }
}

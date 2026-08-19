//! 固定状态栏：在终端顶部显示模型名、Token 用量、审批模式、子代理数等关键信息。
//!
//! 采用"伪固定"策略：初始渲染后，通过 ANSI 光标上移/下移在原位置重写，
//! 无需全屏重绘即可实现视觉上固定的状态栏。

use std::io::{self, Write};

use crate::ui::theme::active_theme;

/// 状态栏渲染上下文：包含所有需要从外部传入的动态字段。
pub struct StatusBar {
    pub model: String,
    pub total_tokens: usize,
    pub max_tokens: usize,
    pub no_approval: bool,
    pub subagents_running: usize,
    pub working_dir: String,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            model: String::from("—"),
            total_tokens: 0,
            max_tokens: 0,
            no_approval: false,
            subagents_running: 0,
            working_dir: String::new(),
        }
    }

    /// 渲染状态栏到 stdout。
    ///
    /// 首次调用时使用 `render_header()`，后续调用使用 `render_update(line_count)`
    /// 来保持光标位置不变。
    pub fn render(&self, width: usize) -> io::Result<()> {
        let mut stdout = io::stdout();
        let theme = active_theme();

        let token_pct = if self.max_tokens > 0 {
            self.total_tokens as f64 / self.max_tokens as f64 * 100.0
        } else {
            0.0
        };

        // 压力等级颜色：复用 budget 模块的分级逻辑
        let token_color = if token_pct > 80.0 {
            theme.error_fg
        } else if token_pct > 60.0 {
            theme.warning_fg
        } else {
            theme.success_fg
        };

        let approval_color = if self.no_approval {
            theme.success_fg
        } else {
            theme.warning_fg
        };

        let approval_str = if self.no_approval {
            "🔓 OFF"
        } else {
            "🔒 ON"
        };

        let bar = build_status_bar(self.total_tokens, self.max_tokens, 15);

        let line = format!(
            "│ 🤖 {} {}  │  💬 {} / {} ({:.1}%)  {}{}{}  │  {} {} {}  │  📁 {}  │",
            theme.tool_fg,
            self.model,
            format_tokens(self.total_tokens),
            format_tokens(self.max_tokens),
            token_pct,
            token_color,
            bar,
            crate::ui::theme::RESET,
            approval_color,
            approval_str,
            crate::ui::theme::RESET,
            self.working_dir,
        );

        // 右填充到终端宽度
        let display_line = pad_right(&line, width);
        writeln!(stdout, "{}", display_line)?;
        stdout.flush()?;
        Ok(())
    }

    /// 更新状态栏中的部分内容（不重绘整行），返回行数（总是 1）。
    pub fn line_count(&self) -> usize {
        1
    }
}

/// 构建状态栏内的迷你进度条。
fn build_status_bar(total: usize, max: usize, width: usize) -> String {
    let filled = if max > 0 {
        (total as f64 / max as f64 * width as f64).round() as usize
    } else {
        0
    };
    let filled = filled.min(width);
    let empty = width - filled;
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

/// 填充字符串到指定宽度。
fn pad_right(s: &str, width: usize) -> String {
    let w = unicode_width::UnicodeWidthStr::width(s);
    let mut result = s.to_string();
    for _ in 0..width.saturating_sub(w) {
        result.push(' ');
    }
    result
}

/// Token 格式化。
fn format_tokens(t: usize) -> String {
    if t >= 1000 {
        let k = t as f64 / 1000.0;
        if k >= 10.0 {
            format!("{:.0}K", k)
        } else {
            format!("{:.1}K", k)
        }
    } else {
        format!("{}B", t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_values() {
        assert_eq!(format_tokens(12400), "12K");
        assert_eq!(format_tokens(2340), "2.3K");
        assert_eq!(format_tokens(900), "900B");
        assert_eq!(format_tokens(262144), "262K");
    }

    #[test]
    fn build_status_bar_all_full() {
        let bar = build_status_bar(100, 100, 10);
        assert!(bar.chars().all(|c| c == '█'));
        assert_eq!(bar.chars().count(), 10);
    }

    #[test]
    fn build_status_bar_all_empty() {
        let bar = build_status_bar(0, 100, 10);
        assert!(bar.chars().all(|c| c == '░'));
    }

    #[test]
    fn build_status_bar_half() {
        let bar = build_status_bar(50, 100, 10);
        let filled = bar.chars().filter(|c| *c == '█').count();
        let empty = bar.chars().filter(|c| *c == '░').count();
        assert_eq!(filled + empty, 10);
        assert!(filled >= 4 && filled <= 6);
    }
}

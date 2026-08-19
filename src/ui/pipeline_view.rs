//! 流水线阶段可视化：将 6 阶段流水线渲染为终端进度条。
//!
//! 格式：
//! ```
//! 🏗 架构设计 ✅ → 💻 代码实现 ✅ → 🧪 测试验证 ⏳ → 🔍 代码审查 ⬜ → 🔧 问题修复 ⬜ → 📋 进度记录 ⬜
//! ───────────────────────────────── 当前: 🧪 测试验证 (3/6) ─────────────────
//! ```

use std::io::{self, IsTerminal, Write};

use crate::agent::pipeline_context::PipelineContext;
use crate::ui::theme::active_theme;

/// 阶段渲染结构。
struct Stage {
    name: String,
    icon: char,
    status: StageStatus,
}

/// 阶段状态。
#[derive(Clone, Copy)]
enum StageStatus {
    Done,
    InProgress,
    Pending,
    Failed,
}

impl StageStatus {
    fn label(&self) -> &'static str {
        match self {
            StageStatus::Done => "✅",
            StageStatus::InProgress => "⏳",
            StageStatus::Pending => "⬜",
            StageStatus::Failed => "❌",
        }
    }
}

/// 渲染流水线阶段可视化到 stdout。
///
/// `current_stage_index` 是当前阶段索引（0-based），`total_stages` 是总阶段数。
/// 阶段名称取自 PipelineContext。
pub fn render_pipeline_progress(context: &PipelineContext, width: usize) -> io::Result<()> {
    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        return Ok(());
    }

    let theme = active_theme();
    let total = context.stages.len();
    if total == 0 {
        return Ok(());
    }

    // 当前阶段索引：PipelineContext 当前在哪个阶段
    let current_idx = context.current_stage.min(total - 1);

    let mut stage_names: Vec<String> = Vec::new();
    let mut stage_status: Vec<StageStatus> = Vec::new();

    for (i, stage) in context.stages.iter().enumerate() {
        stage_names.push(stage.stage_name.clone());

        if i < current_idx {
            // 已完成的阶段
            if !stage.summary.is_empty() || !stage.artifacts.is_empty() {
                stage_status.push(StageStatus::Done);
            } else {
                stage_status.push(StageStatus::Pending);
            }
        } else if i == current_idx {
            stage_status.push(StageStatus::InProgress);
        } else {
            stage_status.push(StageStatus::Pending);
        }
    }

    let icons: [char; 6] = ['🏗', '💻', '🧪', '🔍', '🔧', '📋'];

    // 构建流水线行
    let mut pipeline_line = String::new();
    for (i, name) in stage_names.iter().enumerate() {
        if i > 0 {
            pipeline_line.push_str(" → ");
        }
        let icon = icons.get(i).copied().unwrap_or('●');
        let status = &stage_status[i];

        // 根据状态着色
        let color = match status {
            StageStatus::Done => theme.success_fg,
            StageStatus::InProgress => theme.tool_fg,
            StageStatus::Pending => theme.muted_fg,
            StageStatus::Failed => theme.error_fg,
        };

        pipeline_line.push_str(&format!(
            "{}{icon} {} {}{}",
            color,
            name,
            status.label(),
            crate::ui::theme::RESET
        ));
    }

    // 截断到终端宽度（如果太长）
    let display_line = truncate_to_width(&pipeline_line, width);
    writeln!(stdout, "{}", display_line)?;

    // 进度指示行
    let progress_hint = format!(
        "───────────── 当前: {} ({}{}) {} ({}{})/{} ───────────",
        theme.tool_fg,
        stage_names[current_idx],
        stage_status[current_idx].label(),
        theme.tool_fg,
        current_idx + 1,
        total,
        crate::ui::theme::RESET,
    );

    let display_hint = truncate_to_width(&progress_hint, width);
    writeln!(stdout, "{}", display_hint)?;

    stdout.flush()?;
    Ok(())
}

/// 将文本截断到指定宽度（按 Unicode 宽度计算）。
fn truncate_to_width(s: &str, max_width: usize) -> String {
    let w = unicode_width::UnicodeWidthStr::width(s);
    if w <= max_width {
        s.to_string()
    } else {
        let mut result = String::new();
        let mut current_width = 0;
        for c in s.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(1);
            if current_width + cw > max_width - 3 {
                result.push_str("...");
                break;
            }
            result.push(c);
            current_width += cw;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_to_width_short() {
        let s = "hello";
        assert_eq!(truncate_to_width(s, 10), "hello");
    }

    #[test]
    fn truncate_to_width_long() {
        let s = "a".repeat(50);
        let result = truncate_to_width(&s, 10);
        assert!(result.contains("..."));
    }

    #[test]
    fn truncate_to_width_with_wide_chars() {
        let s = "Hello 你好"; // 8 ASCII + 2 CJK wide = 12 width
        let result = truncate_to_width(s, 10);
        // Should truncate
        assert!(result.len() <= 12);
    }
}

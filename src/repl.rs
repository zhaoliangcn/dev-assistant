//! 交互式 REPL 的 slash 命令分发。
//!
//! 主循环逻辑位于 [`crate::app`]；本模块只负责解析 `/` 开头的命令。

use std::path::Path;

use crate::agent::Agent;
use crate::utils::message_level::MessageLevel;

/// 状态持久化文件名（相对于工作目录）。
pub const STATE_FILE: &str = ".dev-assistant-state.json";

/// slash 命令处理结果。
pub enum SlashOutcome {
    /// 命令已处理完毕，REPL 应继续下一轮读取
    Continue,
    /// 用户请求退出
    Quit,
}

/// 处理一条 slash 命令（以 `/` 开头）。
///
/// 返回 `None` 表示输入不是 slash 命令，调用方应按普通消息处理。
pub fn handle_slash(
    input: &str,
    agent: &mut Agent<'_>,
    working_dir: &Path,
) -> Option<SlashOutcome> {
    if input == "/exit" || input == "/quit" {
        print!("\x1b[2J\x1b[H");
        println!("👋 Goodbye!");
        return Some(SlashOutcome::Quit);
    }

    if input == "/clear" {
        agent.context.display.clear_messages();
        agent.context.display.history_start = agent.context.history.len();
        return Some(SlashOutcome::Continue);
    }

    if input.starts_with("/model") {
        return Some(handle_model_command(input, agent, working_dir));
    }

    None
}

fn handle_model_command(
    input: &str,
    agent: &mut Agent<'_>,
    working_dir: &Path,
) -> SlashOutcome {
    let parts: Vec<&str> = input.split_whitespace().collect();

    if parts.len() == 1 {
        // 列出所有模型，标记当前活跃模型
        let active = agent.active_model().to_string();
        let models: Vec<String> = agent.list_models().into_iter().map(|s| s.to_string()).collect();
        agent.context.add_display_message(
            MessageLevel::Info,
            "可用模型:",
        );
        for m in &models {
            let marker = if m.as_str() == active.as_str() { "→" } else { " " };
            agent.context.add_display_message(
                MessageLevel::Info,
                &format!("{} {}", marker, m),
            );
        }
        return SlashOutcome::Continue;
    }

    // 切换模型
    let model_name = parts[1];
    match agent.switch_model(model_name) {
        Ok(()) => {
            agent.context.add_display_message(
                MessageLevel::Success,
                &format!("切换到模型: {}", model_name),
            );
            agent.context.active_model = Some(model_name.to_string());
            // 立即保存状态
            let state_path = working_dir.join(STATE_FILE);
            let _ = agent.context.save_state(&state_path);
        }
        Err(e) => {
            agent.context.add_display_message(
                MessageLevel::Error,
                &format!("切换失败: {}", e),
            );
        }
    }
    SlashOutcome::Continue
}

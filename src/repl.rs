//! 交互式 REPL：slash 命令分发 + 主循环辅助函数。

use std::io::Write;
use std::path::Path;

use crate::agent::{Agent, AgentStep};
use crate::session::SessionLogger;
use crate::ui::{self, UIMessageOutput, MarkdownRenderer};
use crate::utils::message_level::MessageLevel;
use crate::utils::message_output::MessageOutput;
use crate::utils::error::AppError;

/// 状态持久化文件名（相对于工作目录）。
pub const STATE_FILE: &str = ".dev-assistant-state.json";

/// slash 命令处理结果。
pub enum SlashOutcome {
    /// 命令已处理完毕，REPL 应继续下一轮读取
    Continue,
    /// 用户请求退出
    Quit,
}

/// REPL 主循环每轮的动作。
pub enum ReplAction {
    Continue,
    Quit,
}

/// 处理一条 slash 命令（以 `/` 开头）。
///
/// 返回 `None` 表示输入不是 slash 命令，调用方应按普通消息处理。
pub fn handle_slash(
    input: &str,
    agent: &mut Agent,
    working_dir: &Path,
) -> Option<SlashOutcome> {
    if input == "/exit" || input == "/quit" {
        print!("\x1b[2J\x1b[H");
        println!("👋 Goodbye!");
        return Some(SlashOutcome::Quit);
    }

    if input == "/clear" {
        agent.clear_display_to(agent.history_len());
        return Some(SlashOutcome::Continue);
    }

    if input.starts_with("/model") {
        return Some(handle_model_command(input, agent, working_dir));
    }

    if input == "/status" {
        return Some(handle_status_command(agent));
    }

    if input == "/background" {
        return Some(handle_background_command(agent));
    }

    None
}

fn handle_model_command(
    input: &str,
    agent: &mut Agent,
    working_dir: &Path,
) -> SlashOutcome {
    let parts: Vec<&str> = input.split_whitespace().collect();

    if parts.len() == 1 {
        // 列出所有模型，标记当前活跃模型
        let active = agent.active_model().to_string();
        let models: Vec<String> = agent.list_models().into_iter().map(|s| s.to_string()).collect();
        agent.add_display_message(
            MessageLevel::Info,
            "可用模型:",
        );
        for m in &models {
            let marker = if m.as_str() == active.as_str() { "→" } else { " " };
            agent.add_display_message(
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
            agent.add_display_message(
                MessageLevel::Success,
                &format!("切换到模型: {}", model_name),
            );
            agent.set_active_model(model_name.to_string());
            // 立即保存状态
            let state_path = working_dir.join(STATE_FILE);
            let _ = agent.save_state(&state_path);
        }
        Err(e) => {
            agent.add_display_message(
                MessageLevel::Error,
                &format!("切换失败: {}", e),
            );
        }
    }
    SlashOutcome::Continue
}

fn handle_status_command(agent: &mut Agent) -> SlashOutcome {
    use crate::tools::task_tools::get_global_task_manager;
    
    if let Some(manager) = get_global_task_manager() {
        let graph_arc = manager.graph();
        let graph = graph_arc.lock().unwrap();
        let summary = graph.progress_summary();
        let total = graph.total_count();
        let completed = graph.completed_count();
        drop(graph);
        agent.add_display_message(
            MessageLevel::Info,
            &format!(
                "📊 任务状态:\n\
                 - 总任务数: {}\n\
                 - 已完成: {}\n\
                 \n{}",
                total,
                completed,
                summary,
            ),
        );
    } else {
        agent.add_display_message(
            MessageLevel::Info,
            "当前没有正在运行的后台任务",
        );
    }
    SlashOutcome::Continue
}

fn handle_background_command(agent: &mut Agent) -> SlashOutcome {
    agent.add_display_message(
        MessageLevel::Info,
        "⚠️ 后台模式需要通过命令行参数 --background 启动\n\
         使用方式: dev-assistant --background\n\
         \n\
         在后台模式下，任务将自动执行并定期保存检查点。\n\
         支持的命令:\n\
         - /status: 查询任务状态\n\
         - /pause: 暂停任务\n\
         - /resume: 恢复任务\n\
         - /cancel: 取消任务",
    );
    SlashOutcome::Continue
}

/// 处理一次用户消息：清空展示缓冲区、运行 agent step 循环、刷新 UI。
///
/// 从 [`crate::app::App`] 外提的交互逻辑，让 App 仅保留组件组装职责。
pub async fn process_user_message(
    agent: &mut Agent,
    input: &str,
    session_log: &mut SessionLogger,
    working_dir: &Path,
    restart_args: &[String],
    verbose: bool,
    markdown_renderer: &MarkdownRenderer,
) -> Result<ReplAction, AppError> {
    // Clear stale display messages from previous turn so they
    // don't accumulate and stack on each render.
    agent.reset_display_for_new_turn();

    // ── Step-by-step agent loop with real-time UI updates ──
    let mut output = UIMessageOutput::new(verbose);
    session_log.log_user(input);
    agent.start_turn(input.to_string(), &mut output);

    // 渲染用户消息
    let user_block = ui::MessageBlock::User {
        content: input.to_string(),
    };
    ui::render_block(&user_block, markdown_renderer)?;

    let result = loop {
        // Drain buffered messages and render blocks
        for (level, msg) in output.drain() {
            let label = level.label();
            session_log.log_status(label, &msg);
            agent.add_display_message(level, &msg);
            
            // 根据消息级别渲染不同类型的块
            let block = match level {
                MessageLevel::Error => ui::MessageBlock::Error { content: msg },
                MessageLevel::Warning => ui::MessageBlock::System { content: format!("⚠️ {}", msg) },
                MessageLevel::Info => {
                    // 检测消息内容，尝试分类
                    if msg.starts_with("💭") || msg.contains("思考") || msg.contains("thinking") {
                        ui::MessageBlock::Thinking { content: msg }
                    } else if msg.starts_with("🔧") || msg.contains("工具") {
                        ui::MessageBlock::System { content: msg }
                    } else {
                        ui::MessageBlock::System { content: format!("ℹ️ {}", msg) }
                    }
                }
                MessageLevel::Debug => ui::MessageBlock::System { content: format!("🐛 {}", msg) },
                MessageLevel::Success => ui::MessageBlock::ToolResult {
                    tool_name: "操作".to_string(),
                    success: true,
                    content: msg,
                },
            };
            ui::render_block(&block, markdown_renderer)?;
        }

        // Show "thinking" indicator in the input area
        session_log.log_thinking();
        ui::render_input_panel(Some("⏳ LLM 正在思考，请稍候..."))?;

        tokio::select! {
            step_result = agent.step(&mut output) => {
                match step_result {
                    Ok(AgentStep::Done(result)) => break Some(result),
                    Ok(AgentStep::Continue) => continue,
                    Err(e) => {
                        let msg = format!("LLM API 错误: {}", e);
                        output.error(&msg);
                        session_log.log_status("错误", &msg);
                        break None;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                output.info("操作已取消");
                session_log.log_status("警告", "用户中断了当前操作");
                break None;
            }
        }
    };

    // Flush remaining messages
    for (level, msg) in output.drain() {
        let label = level.label();
        session_log.log_status(label, &msg);
        agent.add_display_message(level, &msg);
    }

    // 处理用户中断的情况：回到输入提示，不处理结果
    let result = match result {
        Some(r) => r,
        None => {
            agent.add_display_message(
                MessageLevel::Warning,
                "⏹ 操作已取消",
            );
            return Ok(ReplAction::Continue);
        }
    };

    // Add result to conversation history so it appears at the end
    // of the message list, not just as a status message at the top.
    agent.add_message(
        crate::agent::context::Role::Assistant,
        result.message.clone(),
        None,
        None,
    );
    session_log.log_assistant(&result.message);

    // 持久化：记录最终助手消息（如果 step 循环中尚未记录）
    agent.record_assistant_message_to_store(&result.message);

    // Handle restart request
    if result.restart_requested {
        return handle_restart(agent, working_dir, restart_args, verbose);
    }

    Ok(ReplAction::Continue)
}

/// 处理 /pipeline 命令：启动多阶段流水线（设计→编码→审查→修复→记录）。
///
/// 每个阶段创建一个对应身份的子 Agent，上一个阶段的输出作为
/// 下一个阶段的上下文传入。
#[allow(dead_code)]
pub async fn handle_pipeline_command(
    agent: &mut Agent,
    task: &str,
    verbose: bool,
) -> Result<ReplAction, AppError> {
    agent.add_display_message(
        MessageLevel::Info,
        &format!("🚀 启动流水线: {}", task),
    );

    agent.run_pipeline(task, verbose).await?;

    Ok(ReplAction::Continue)
}

/// 处理 restart 请求：保存状态、cargo build、exec 替换进程。
///
/// 返回 [`ReplAction::Continue`] 表示构建失败、继续 REPL；
/// 返回 [`ReplAction::Quit`] 表示 exec 成功或失败后退出。
pub fn handle_restart(
    agent: &mut Agent,
    working_dir: &Path,
    restart_args: &[String],
    verbose: bool,
) -> Result<ReplAction, AppError> {
    use crate::restart::perform_restart;

    let state_path = working_dir.join(STATE_FILE);
    if let Err(e) = agent.save_state(&state_path) {
        agent.add_display_message(
            MessageLevel::Error,
            &format!("保存状态失败: {}。未重启。", e),
        );
        let messages = agent.get_display_messages();
        ui::render(&messages, &agent.display_messages(), None, verbose)?;
        return Ok(ReplAction::Quit);
    }

    agent.add_display_message(
        MessageLevel::Info,
        "正在运行 cargo build...",
    );
    let messages = agent.get_display_messages();
    ui::render(&messages, &agent.display_messages(), None, verbose)?;
    std::io::stdout().flush().ok();

    // perform_restart 会在成功时 exec() 替换进程，永远不会返回；
    // 返回 true 表示构建失败、需要继续 REPL。
    let should_continue = perform_restart(
        working_dir,
        restart_args,
        &mut |level, msg: String| {
            agent.add_display_message(level, &msg);
        },
    );

    if should_continue {
        let messages = agent.get_display_messages();
        ui::render(&messages, &agent.display_messages(), None, verbose)?;
        Ok(ReplAction::Continue)
    } else {
        Ok(ReplAction::Quit)
    }
}

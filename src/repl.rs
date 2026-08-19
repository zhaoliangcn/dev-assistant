//! 交互式 REPL：slash 命令分发 + 主循环辅助函数。

use std::io::{self, Write};
use std::path::Path;

use crate::agent::{Agent, AgentStep};
use crate::ui::{self, MarkdownRenderer, UIMessageOutput};
use crate::utils::error::AppError;
use crate::utils::message_level::MessageLevel;
use crate::utils::message_output::MessageOutput;

// ── 状态指示器 ─────────────────────────────────────────────────────────

/// 根据上一轮输出的最后一条消息，推导下一步的上下文状态提示。
///
/// 使用 [`crate::ui::StatusType`] 进行分类，确保与状态栏显示语义一致。
fn derive_thinking_status(last_msg: Option<&str>) -> &'static str {
    let msg = match last_msg {
        Some(m) => m,
        None => return "LLM 正在思考...",
    };

    match crate::ui::StatusType::from(msg) {
        crate::ui::StatusType::ToolCall => "正在执行工具调用...",
        crate::ui::StatusType::WaitingLLM => "等待 LLM 响应...",
        crate::ui::StatusType::Analyzing => "正在分析代码...",
        crate::ui::StatusType::Reading => "正在读取文件...",
        crate::ui::StatusType::Done => "处理完成，生成回复中...",
        crate::ui::StatusType::Other => "LLM 正在思考...",
    }
}

/// 合并连续相同类型的消息块
///
/// 例如：连续 15 次 "🔧 ReadFile" 调用合并为一条 "🔧 ReadFile (×15)"
fn merge_consecutive_blocks(blocks: &[ui::MessageBlock]) -> Vec<ui::MessageBlock> {
    if blocks.is_empty() {
        return Vec::new();
    }

    // 只有一个块时直接返回原块，避免 Debug 格式泄露
    if blocks.len() == 1 {
        return blocks.to_vec();
    }

    let mut result = Vec::new();
    let mut current_run: Vec<&ui::MessageBlock> = Vec::new();

    for block in blocks {
        let block_type_key = block.block_type();

        if let Some(&first) = current_run.first() {
            if first.block_type() == block_type_key {
                current_run.push(block);
                continue;
            }

            // 不同类型的块：先处理当前批次
            if current_run.len() > 1 {
                result.push(current_run[0].with_merge_count(current_run.len()));
            } else {
                result.push((*current_run[0]).clone());
            }
            current_run.clear();
        }
        current_run.push(block);
    }

    // 处理最后一个批次
    if !current_run.is_empty() {
        if current_run.len() > 1 {
            result.push(current_run[0].with_merge_count(current_run.len()));
        } else {
            result.push((*current_run[0]).clone());
        }
    }

    result
}

/// 优先通过前导 emoji 判断，避免字符串误匹配。
fn classify_info_message(msg: &str) -> ui::MessageBlock {
    // 按 emoji 前缀精确判断
    if msg.starts_with("💭") {
        ui::MessageBlock::Thinking {
            content: msg.to_string(),
        }
    } else if msg.starts_with("🔧") || msg.starts_with("↻") {
        ui::MessageBlock::System {
            content: msg.to_string(),
        }
    } else if msg.starts_with("✅") {
        ui::MessageBlock::ToolResult {
            tool_name: String::new(),
            success: true,
            content: msg.to_string(),
        }
    } else if msg.starts_with("❌") || msg.starts_with("🔥") {
        ui::MessageBlock::Error {
            content: msg.to_string(),
        }
    } else if msg.starts_with("⚠️") || msg.starts_with("ℹ️") || msg.starts_with("📝") {
        ui::MessageBlock::System {
            content: msg.to_string(),
        }
    } else {
        // 兜底：无 emoji 时再尝试文本匹配
        let is_thinking = msg.contains("思考") || msg.contains("thinking") || msg.contains("分析");
        let is_tool = msg.contains("工具") || msg.contains("执行");
        if is_thinking {
            ui::MessageBlock::Thinking {
                content: msg.to_string(),
            }
        } else if is_tool {
            ui::MessageBlock::System {
                content: msg.to_string(),
            }
        } else {
            ui::MessageBlock::System {
                content: format!("ℹ️ {}", msg),
            }
        }
    }
}

/// 刷新待处理的 Thinking 合并块。
/// 如果有累积的 Thinking 消息，渲染为合并后的一条。
fn flush_pending_thinking(
    pending: &mut Option<String>,
    count: &mut usize,
    markdown_renderer: &MarkdownRenderer,
) -> io::Result<()> {
    if let Some(latest_msg) = pending.take() {
        let merged = if *count > 1 {
            format!("{} ↩ ({} 条更新)", latest_msg, count)
        } else {
            latest_msg
        };
        let block = ui::MessageBlock::Thinking { content: merged };
        ui::render_block(&block, markdown_renderer)?;
        *count = 0;
    }
    Ok(())
}

/// 状态持久化文件名（相对于工作目录）。
pub const STATE_FILE: &str = ".dev-assistant-state.json";

/// slash 命令处理结果。
pub enum SlashOutcome {
    /// 命令已处理完毕，REPL 应继续下一轮读取
    Continue,
}

/// REPL 主循环每轮的动作。
pub enum ReplAction {
    Continue,
    Quit,
}

/// 处理应用级 slash 命令（`/status`、`/background`、`/schedule` 等）。
///
/// 注意：`/model` 命令已在 `app.rs` 中处理，此处不再重复。
/// `/exit`、`/quit`、`/clear`、`/help` 等通用命令由 `input::SlashCommand` 处理。
///
/// 所有命令的输出直接通过 `ui::render_block` 渲染到终端，避免存入 `DisplayBuffer`
/// 后被 `reset_display_for_new_turn()` 清空。
pub fn handle_slash(input: &str, agent: &mut Agent, working_dir: &Path) -> Option<SlashOutcome> {
    if input == "/status" {
        return Some(handle_status_command(agent));
    }

    if input == "/budget" {
        return Some(handle_budget_command(agent));
    }

    if input == "/background" {
        return Some(handle_background_command(agent));
    }

    if input.starts_with("/schedule ") || input == "/schedule" {
        return Some(handle_schedule_command(input));
    }

    if input.starts_with("/unschedule ") {
        return Some(handle_unschedule_command(input));
    }

    if input == "/scheduled" || input == "/tasks" {
        return Some(handle_list_scheduled_command());
    }

    if input.starts_with("/skill") {
        return Some(handle_skill_command(input, working_dir));
    }

    None
}

fn handle_status_command(_agent: &mut Agent) -> SlashOutcome {
    let md = MarkdownRenderer::new();
    use crate::tools::task_tools::get_global_task_manager;

    let content = if let Some(manager) = get_global_task_manager() {
        let graph_arc = manager.graph();
        let graph = graph_arc.lock().unwrap();
        let summary = graph.progress_summary();
        let total = graph.total_count();
        let completed = graph.completed_count();
        drop(graph);
        format!(
            "📊 任务状态:\n\
             - 总任务数: {}\n\
             - 已完成: {}\n\
             \n{}",
            total, completed, summary,
        )
    } else {
        "当前没有正在运行的后台任务".to_string()
    };
    let _ = ui::render_block(&ui::MessageBlock::System { content }, &md);
    SlashOutcome::Continue
}

fn handle_background_command(_agent: &mut Agent) -> SlashOutcome {
    let md = MarkdownRenderer::new();
    let content = "⚠️ 后台模式需要通过命令行参数 --background 启动\n\
         使用方式: dev-assistant --background\n\
         \n\
         在后台模式下，任务将自动执行并定期保存检查点。\n\
         支持的命令:\n\
         - /status: 查询任务状态\n\
         - /pause: 暂停任务\n\
         - /resume: 恢复任务\n\
         - /cancel: 取消任务"
        .to_string();
    let _ = ui::render_block(&ui::MessageBlock::System { content }, &md);
    SlashOutcome::Continue
}

/// 处理 `/budget` 命令：显示详细上下文预算面板。
fn handle_budget_command(agent: &mut Agent) -> SlashOutcome {
    let budget = agent.get_budget_report();
    let _ = ui::render_budget_detail(&budget);
    SlashOutcome::Continue
}

// ── 定时任务命令 ─────────────────────────────────────────────────

/// 处理 `/schedule` 命令：创建定时任务。
///
/// 用法:
///   /schedule cron "<表达式>" agent <指令>
///   /schedule interval <秒> command <命令>
///   /schedule once <秒> agent <指令>
fn handle_schedule_command(input: &str) -> SlashOutcome {
    use crate::scheduler::task::{ScheduleType, ScheduledTask, TaskExecutionMode};
    use crate::scheduler::tools_handlers::get_global_scheduler;

    let md = MarkdownRenderer::new();
    let scheduler = match get_global_scheduler() {
        Some(s) => s,
        None => {
            let _ = ui::render_block(
                &ui::MessageBlock::Error {
                    content: "❌ 调度器未初始化".to_string(),
                },
                &md,
            );
            return SlashOutcome::Continue;
        }
    };

    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.len() < 4 {
        let _ = ui::render_block(
            &ui::MessageBlock::Error {
                content: "❌ 用法:\n  /schedule cron \"<表达式>\" agent <指令>\n  /schedule interval <秒> command <命令>\n  /schedule once <秒> agent <指令>".to_string(),
            },
            &md,
        );
        return SlashOutcome::Continue;
    }

    let schedule_type = parts[1];
    let schedule = match schedule_type {
        "cron" => {
            // 合并剩余参数直到 agent/command
            let expr = parts[2..]
                .iter()
                .take_while(|p| **p != "agent" && **p != "command")
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            if expr.is_empty() {
                let _ = ui::render_block(
                    &ui::MessageBlock::Error {
                        content: "❌ cron 表达式不能为空".to_string(),
                    },
                    &md,
                );
                return SlashOutcome::Continue;
            }
            ScheduleType::Cron(expr)
        }
        "interval" => {
            let secs: u64 = match parts[2].parse() {
                Ok(s) => s,
                Err(_) => {
                    let _ = ui::render_block(
                        &ui::MessageBlock::Error {
                            content: "❌ 间隔秒数必须为数字".to_string(),
                        },
                        &md,
                    );
                    return SlashOutcome::Continue;
                }
            };
            ScheduleType::Interval(secs)
        }
        "once" => {
            let secs: u64 = match parts[2].parse() {
                Ok(s) => s,
                Err(_) => {
                    let _ = ui::render_block(
                        &ui::MessageBlock::Error {
                            content: "❌ 延迟秒数必须为数字".to_string(),
                        },
                        &md,
                    );
                    return SlashOutcome::Continue;
                }
            };
            ScheduleType::Once(secs)
        }
        _ => {
            let _ = ui::render_block(
                &ui::MessageBlock::Error {
                    content: format!(
                        "❌ 未知调度类型: {}，支持: cron/interval/once",
                        schedule_type
                    ),
                },
                &md,
            );
            return SlashOutcome::Continue;
        }
    };

    // 找到 agent/command 关键字的位置
    let mode_keyword_idx = parts.iter().position(|p| *p == "agent" || *p == "command");
    let mode_idx = match mode_keyword_idx {
        Some(i) => i,
        None => {
            let _ = ui::render_block(
                &ui::MessageBlock::Error {
                    content: "❌ 缺少执行模式 (agent/command)".to_string(),
                },
                &md,
            );
            return SlashOutcome::Continue;
        }
    };
    let mode_type = parts[mode_idx]; // "agent" or "command"

    // 合并剩余参数作为指令/命令内容
    let remaining: String = parts[mode_idx + 1..].join(" ");
    if remaining.is_empty() {
        let _ = ui::render_block(
            &ui::MessageBlock::Error {
                content: format!(
                    "❌ {} 指令内容不能为空",
                    if mode_type == "agent" {
                        "Agent"
                    } else {
                        "命令"
                    }
                ),
            },
            &md,
        );
        return SlashOutcome::Continue;
    }

    let mode = match mode_type {
        "agent" => TaskExecutionMode::Agent {
            instruction: remaining.clone(),
        },
        "command" => TaskExecutionMode::Command {
            command: remaining.clone(),
            working_dir: None,
        },
        _ => unreachable!(),
    };

    let task_id = format!(
        "sched_{}_{}",
        chrono::Utc::now().format("%Y%m%d%H%M%S"),
        crate::scheduler::tools::generate_short_id(6)
    );

    let task_name = format!(
        "{}: {}",
        mode_type,
        remaining.chars().take(30).collect::<String>()
    );
    let task = ScheduledTask::new(task_id, task_name, schedule, mode, 3, vec![], 600);

    match scheduler.schedule_task(&task) {
        Ok(()) => {
            let _ = ui::render_block(
                &ui::MessageBlock::ToolResult {
                    tool_name: "定时任务".to_string(),
                    success: true,
                    content: format!("✅ 已创建定时任务: {} (ID: {})", task.name, task.id),
                },
                &md,
            );
        }
        Err(e) => {
            let _ = ui::render_block(
                &ui::MessageBlock::Error {
                    content: format!("❌ 创建失败: {}", e),
                },
                &md,
            );
        }
    }
    SlashOutcome::Continue
}

/// 处理 `/unschedule` 命令：取消定时任务。
///
/// 用法: /unschedule <task-id>
fn handle_unschedule_command(input: &str) -> SlashOutcome {
    use crate::scheduler::tools_handlers::get_global_scheduler;

    let md = MarkdownRenderer::new();
    let scheduler = match get_global_scheduler() {
        Some(s) => s,
        None => {
            let _ = ui::render_block(
                &ui::MessageBlock::Error {
                    content: "❌ 调度器未初始化".to_string(),
                },
                &md,
            );
            return SlashOutcome::Continue;
        }
    };

    let task_id = input
        .strip_prefix("/unschedule ")
        .unwrap_or("")
        .trim()
        .to_string();
    if task_id.is_empty() {
        let _ = ui::render_block(
            &ui::MessageBlock::Error {
                content: "❌ 用法: /unschedule <task-id>\n使用 /scheduled 查看任务 ID".to_string(),
            },
            &md,
        );
        return SlashOutcome::Continue;
    }

    match scheduler.unschedule_task(&task_id) {
        Ok(true) => {
            let _ = ui::render_block(
                &ui::MessageBlock::ToolResult {
                    tool_name: "定时任务".to_string(),
                    success: true,
                    content: format!("✅ 已取消任务: {}", task_id),
                },
                &md,
            );
        }
        Ok(false) => {
            let _ = ui::render_block(
                &ui::MessageBlock::Error {
                    content: format!("❌ 未找到任务: {}", task_id),
                },
                &md,
            );
        }
        Err(e) => {
            let _ = ui::render_block(
                &ui::MessageBlock::Error {
                    content: format!("❌ 取消失败: {}", e),
                },
                &md,
            );
        }
    }
    SlashOutcome::Continue
}

/// 处理 `/scheduled` / `/tasks` 命令：列出所有定时任务。
fn handle_list_scheduled_command() -> SlashOutcome {
    use crate::scheduler::tools_handlers::get_global_scheduler;

    let md = MarkdownRenderer::new();
    let scheduler = match get_global_scheduler() {
        Some(s) => s,
        None => {
            let _ = ui::render_block(
                &ui::MessageBlock::Error {
                    content: "❌ 调度器未初始化".to_string(),
                },
                &md,
            );
            return SlashOutcome::Continue;
        }
    };

    // 底层 store 的 get_all_tasks 本身是同步方法（engine 的 async 版本只是
    // 简单包装），直接同步调用，避免 block_in_place（它要求多线程 runtime，
    // 而 main.rs 用的是 current_thread runtime，会 panic）。
    let tasks = match scheduler.store().get_all_tasks() {
        Ok(t) => t,
        Err(e) => {
            let _ = ui::render_block(
                &ui::MessageBlock::Error {
                    content: format!("❌ 查询失败: {}", e),
                },
                &md,
            );
            return SlashOutcome::Continue;
        }
    };

    if tasks.is_empty() {
        let _ = ui::render_block(
            &ui::MessageBlock::System {
                content: "📭 暂无定时任务\n使用 /schedule 创建定时任务".to_string(),
            },
            &md,
        );
        return SlashOutcome::Continue;
    }

    let mut content = format!("📋 定时任务列表 (共 {} 个):\n\n", tasks.len());
    for task in &tasks {
        let schedule_desc = match &task.schedule {
            crate::scheduler::task::ScheduleType::Cron(expr) => format!("cron `{}`", expr),
            crate::scheduler::task::ScheduleType::Interval(secs) => format!("每隔 {} 秒", secs),
            crate::scheduler::task::ScheduleType::Once(secs) => format!("{} 秒后执行一次", secs),
        };
        let mode_desc = match &task.mode {
            crate::scheduler::task::TaskExecutionMode::Agent { instruction } => {
                format!(
                    "Agent: {}",
                    instruction.chars().take(40).collect::<String>()
                )
            }
            crate::scheduler::task::TaskExecutionMode::Command { command, .. } => {
                format!("Command: {}", command.chars().take(40).collect::<String>())
            }
        };
        content.push_str(&format!(
            "  ID: `{}`\n  名称: {}\n  调度: {} | 模式: {}\n  状态: {:?} | 执行: {} 次\n\n",
            task.id, task.name, schedule_desc, mode_desc, task.status, task.run_count,
        ));
    }
    let _ = ui::render_block(&ui::MessageBlock::System { content }, &md);
    SlashOutcome::Continue
}

// ── 消息渲染辅助函数 ────────────────────────────────────────────────

/// 将消息级别和内容转换为 MessageBlock（统一逻辑，避免重复）。
fn message_to_block(level: MessageLevel, msg: String) -> ui::MessageBlock {
    match level {
        MessageLevel::Error => ui::MessageBlock::Error { content: msg },
        MessageLevel::Warning => ui::MessageBlock::System {
            content: format!("⚠️ {}", msg),
        },
        MessageLevel::Info => {
            let b = classify_info_message(&msg);
            if matches!(b, ui::MessageBlock::Thinking { .. }) {
                // Thinking 块需要特殊处理（合并），调用方需检查
                b
            } else {
                b
            }
        }
        MessageLevel::Debug => ui::MessageBlock::System {
            content: format!("🐛 {}", msg),
        },
        MessageLevel::Success => ui::MessageBlock::ToolResult {
            tool_name: "操作".to_string(),
            success: true,
            content: msg,
        },
    }
}

/// 渲染一批消息块（处理连续合并）。
fn render_block_batch(
    blocks: &[ui::MessageBlock],
    markdown_renderer: &MarkdownRenderer,
) -> io::Result<()> {
    if blocks.is_empty() {
        return Ok(());
    }
    let merged_blocks = merge_consecutive_blocks(blocks);
    for merged in merged_blocks {
        ui::render_block(&merged, markdown_renderer)?;
    }
    Ok(())
}

// ── 主循环 ─────────────────────────────────────────────────────────

/// 处理一次用户消息：清空展示缓冲区、运行 agent step 循环、刷新 UI。
///
/// 从 [`crate::app::App`] 外提的交互逻辑，让 App 仅保留组件组装职责。
pub async fn process_user_message(
    agent: &mut Agent,
    input: &str,
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
    agent.start_turn(input.to_string(), &mut output);

    // 渲染用户消息
    let user_block = ui::MessageBlock::User {
        content: input.to_string(),
    };
    ui::render_block(&user_block, markdown_renderer)?;

    let mut step_round: usize = 0;
    // Thinking 块合并状态
    let mut pending_thinking: Option<String> = None;
    let mut thinking_count: usize = 0;
    // 连续相同类型消息合并状态
    let mut last_block_type: Option<(&'static str, std::time::Instant)> = None;
    let mut current_type_blocks: Vec<ui::MessageBlock> = Vec::new();

    let result = loop {
        // 清空上一轮流式渲染留下的活跃流区域（含状态行），
        // 避免其与后续 drain 状态块交错堆叠导致内容残留/覆盖。
        output.clear_active_stream();

        // 渲染上一轮流式完成后的助手内容（避免与 drain/render_block 重复）
        if let Some(assistant_content) = output.take_pending_assistant() {
            flush_pending_thinking(
                &mut pending_thinking,
                &mut thinking_count,
                markdown_renderer,
            )?;
            let block = ui::MessageBlock::Assistant {
                content: assistant_content,
            };
            render_block_batch(&[block], markdown_renderer)?;
        }

        // Drain buffered messages and render blocks
        for (level, msg) in output.drain() {
            agent.add_display_message(level, &msg);

            // 根据消息级别渲染不同类型的块
            let block = message_to_block(level, msg);

            // Thinking 块需要合并处理
            if matches!(block, ui::MessageBlock::Thinking { .. }) {
                pending_thinking = Some(block.content().to_string());
                thinking_count += 1;
                continue;
            }

            flush_pending_thinking(
                &mut pending_thinking,
                &mut thinking_count,
                markdown_renderer,
            )?;

            // 检查是否是连续相同类型的消息（50ms 内）
            let block_type = block.block_type();
            let now = std::time::Instant::now();

            match &last_block_type {
                Some((last_type, last_time))
                    if *last_type == block_type
                        && now.duration_since(*last_time).as_millis() < 50 =>
                {
                    // 相同类型，50ms 内：加入当前批次
                    current_type_blocks.push(block);
                }
                _ => {
                    // 不同类型或超过 50ms：先渲染上一批次
                    render_block_batch(&current_type_blocks, markdown_renderer)?;
                    // 开始新批次
                    current_type_blocks.clear();
                    current_type_blocks.push(block);
                    last_block_type = Some((block_type, now));
                }
            }
        }

        // 刷新本轮剩余的待处理 Thinking 块
        flush_pending_thinking(
            &mut pending_thinking,
            &mut thinking_count,
            markdown_renderer,
        )?;

        // 渲染剩余批次
        render_block_batch(&current_type_blocks, markdown_renderer)?;
        current_type_blocks.clear();

        // 检测上一轮操作，生成上下文状态提示
        let status = derive_thinking_status(output.last_message());
        step_round += 1;
        let spinner = if step_round.is_multiple_of(2) {
            "⏳"
        } else {
            "⌛"
        };
        ui::render_input_panel(Some(&format!("{} {}", spinner, status)))?;

        tokio::select! {
            step_result = agent.step(&mut output) => {
                match step_result {
                    Ok(AgentStep::Done(result)) => break Some(result),
                    Ok(AgentStep::Continue) => continue,
                    Err(e) => {
                        let msg = format!("LLM API 错误: {}", e);
                        output.error(&msg);
                        break None;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                output.info("操作已取消");
                break None;
            }
        }
    };

    // 循环退出（Done/错误/中断）时，清空可能仍活跃的流式区域，
    // 避免残留的旧流式行与下方最终状态块交错堆叠。
    output.clear_active_stream();

    // Flush remaining messages
    for (level, msg) in output.drain() {
        agent.add_display_message(level, &msg);

        // 渲染到终端（与主循环使用相同的分类逻辑）
        let block = message_to_block(level, msg);

        // Thinking 块需要合并处理
        if matches!(block, ui::MessageBlock::Thinking { .. }) {
            pending_thinking = Some(block.content().to_string());
            thinking_count += 1;
            continue;
        }

        flush_pending_thinking(
            &mut pending_thinking,
            &mut thinking_count,
            markdown_renderer,
        )?;

        // 检查是否是连续相同类型的消息（50ms 内）
        let block_type = block.block_type();
        let now = std::time::Instant::now();

        match &last_block_type {
            Some((last_type, last_time))
                if *last_type == block_type && now.duration_since(*last_time).as_millis() < 50 =>
            {
                // 相同类型，50ms 内：加入当前批次
                current_type_blocks.push(block);
            }
            _ => {
                // 不同类型或超过 50ms：先渲染上一批次
                render_block_batch(&current_type_blocks, markdown_renderer)?;
                // 开始新批次
                current_type_blocks.clear();
                current_type_blocks.push(block);
                last_block_type = Some((block_type, now));
            }
        }
    }
    // 刷新本轮剩余的待处理 Thinking 块
    flush_pending_thinking(
        &mut pending_thinking,
        &mut thinking_count,
        markdown_renderer,
    )?;

    // 渲染剩余批次
    render_block_batch(&current_type_blocks, markdown_renderer)?;
    current_type_blocks.clear();

    // 处理用户中断的情况：回到输入提示，不处理结果
    let result = match result {
        Some(r) => r,
        None => {
            agent.add_display_message(MessageLevel::Warning, "⏹ 操作已取消");
            return Ok(ReplAction::Continue);
        }
    };

    // 判断结果是否来自 finish 工具调用（结构化标记，避免字符串前缀误判）。
    // finish 的结果已由 step() 作为工具结果添加到历史（tool role），
    // 并在 drain 阶段作为 ToolResult 块渲染，此处不应再重复添加或渲染。
    let is_finish_result = result.finished;

    if !is_finish_result {
        // 非 finish 结果：添加到对话历史中以在末尾显示，
        // 避免仅作为状态消息出现在顶部。
        agent.add_message(
            crate::agent::context::Role::Assistant,
            result.message.clone(),
            None,
            None,
        );
    } else {
        // finish 结果无需额外处理（已作为工具结果渲染）
    }

    // 渲染上一轮流式完成后的助手内容（未在循环中消费时，如 Done 后直接跳出）
    let last_assistant = output.take_pending_assistant();
    if let Some(ref content) = last_assistant {
        let block = ui::MessageBlock::Assistant {
            content: content.clone(),
        };
        ui::render_block(&block, markdown_renderer)?;
    }

    // 渲染最终结果：仅当与已渲染的助手内容不同时才渲染，避免重复。
    // finish 结果已作为 ToolResult 在 drain 阶段渲染，不再重复渲染。
    if !is_finish_result
        && result.message != last_assistant.unwrap_or_default()
        && !result.message.is_empty()
    {
        let result_block = ui::MessageBlock::Assistant {
            content: result.message.clone(),
        };
        ui::render_block(&result_block, markdown_renderer)?;
    }

    // 渲染累计的 Token 消耗统计：独立于交互消息流，统一在末尾展示
    if let Some((prompt, completion, total)) = output.take_token_usage() {
        let msg = format!(
            "🔤 Token 消耗: prompt={} · completion={} · total={}",
            prompt, completion, total
        );
        let block = ui::MessageBlock::System { content: msg };
        ui::render_block(&block, markdown_renderer)?;
    }

    // 持久化：记录最终助手消息（如果 step 循环中尚未记录）
    agent.record_assistant_message_to_store(&result.message);

    // Handle restart request
    if result.restart_requested {
        // exec 替换进程前先刷盘，避免缓冲区中的事件在 exec 时丢失
        agent.flush_persistence();
        return handle_restart(agent, working_dir, restart_args, verbose);
    }

    // 回合结束：刷盘，保证并发读者（Web 会话详情、背景 ingest）可见完整数据
    agent.flush_persistence();

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
        render_agent_messages(agent, verbose)?;
        return Ok(ReplAction::Quit);
    }

    agent.add_display_message(MessageLevel::Info, "正在运行 cargo build...");
    render_agent_messages(agent, verbose)?;
    std::io::stdout().flush().ok();

    // perform_restart 会在成功时 exec() 替换进程，永远不会返回；
    // 返回 true 表示构建失败、需要继续 REPL。
    let should_continue = perform_restart(working_dir, restart_args, &mut |level, msg: String| {
        agent.add_display_message(level, &msg);
    });

    if should_continue {
        render_agent_messages(agent, verbose)?;
        Ok(ReplAction::Continue)
    } else {
        Ok(ReplAction::Quit)
    }
}

/// 将 agent 的显示消息转换为 MessageBlock 并渲染（新 API）。
pub fn render_agent_messages(agent: &Agent, verbose: bool) -> Result<(), AppError> {
    let md = MarkdownRenderer::new();
    let mut blocks: Vec<ui::MessageBlock> = Vec::new();

    for (role, content) in agent.get_display_messages() {
        blocks.push(ui::MessageBlock::from((role.as_str(), content.as_str())));
    }
    // 也包含状态消息（非 verbose 模式下跳过 Debug/Info）
    for (role, content) in agent.display_messages() {
        if !verbose && (role == "调试" || role == "信息") {
            continue;
        }
        blocks.push(ui::MessageBlock::from((role.as_str(), content.as_str())));
    }

    ui::render_blocks(&blocks, &md)?;

    // 每轮渲染末尾显示紧凑上下文预算条（复用已有 ContextBudget API）。
    // 非终端输出时 render_budget_bar 内部静默跳过，不影响管道/重定向。
    let _ = ui::render_budget_bar(&agent.get_budget_report());

    Ok(())
}

/// 处理 `/skill` slash 命令：安装/列表/移除技能。
///
/// 语法：
///   /skill add <source> [--skill <name>] [--global]
///   /skill list [--global]
///   /skill remove <name> [--global]
///   /skill update [--global]
fn handle_skill_command(input: &str, working_dir: &Path) -> SlashOutcome {
    use crate::skills::installer::{
        install_skill, list_skills, read_skill_meta, remove_skill, InstallScope,
    };

    let md = MarkdownRenderer::new();
    let parts: Vec<&str> = input.split_whitespace().collect();

    if parts.len() < 2 {
        let _ = ui::render_block(
            &ui::MessageBlock::System {
                content: "📚 技能命令用法：\n\
                    `/skill add <source> [--skill <name>] [--global]`  — 安装技能\n\
                    `/skill list [--global]`  — 列出已安装技能\n\
                    `/skill remove <name> [--global]`  — 移除技能\n\
                    `/skill update [--global]`  — 更新技能\n\n\
                    source 格式：\n\
                    `owner/repo`  — Git 仓库（展开为 GitHub URL）\n\
                    `https://github.com/...`  — 完整 Git URL\n\
                    `./local-path`  — 本地目录"
                    .to_string(),
            },
            &md,
        );
        return SlashOutcome::Continue;
    }

    match parts[1] {
        "add" => {
            if parts.len() < 3 {
                let _ = ui::render_block(
                    &ui::MessageBlock::Error {
                        content:
                            "❌ 缺少源参数。用法：/skill add <source> [--skill <name>] [--global]"
                                .to_string(),
                    },
                    &md,
                );
                return SlashOutcome::Continue;
            }

            let source = parts[2];
            let is_global = parts.contains(&"--global");
            let skill_filters: Vec<String> = parts
                .iter()
                .skip(3)
                .filter(|p| **p != "--global")
                .map(|p| p.to_string())
                .collect();

            let scope = if is_global {
                InstallScope::Global
            } else {
                InstallScope::Project
            };

            // skill add 命令在同步上下文中执行，但 install_skill 是 async
            // 使用 tokio Handle 直接在当前运行时执行
            let working_dir = working_dir.to_path_buf();
            let source = source.to_string();
            let skill_filters = skill_filters.clone();
            let result = std::thread::spawn(move || {
                tokio::runtime::Handle::current().block_on(async {
                    install_skill(&source, scope, &working_dir, Some(&skill_filters)).await
                })
            })
            .join()
            .unwrap();

            match result {
                Ok(ref skills) => {
                    let names: Vec<String> = skills.iter().map(|s| s.meta.name.clone()).collect();
                    let mut content =
                        format!("✅ 已安装 {} 个技能（{} 范围）：\n", names.len(), scope);
                    for skill in skills {
                        content.push_str(&format!(
                            "  • **{}**: {}\n",
                            skill.meta.name, skill.meta.description
                        ));
                    }
                    let _ = ui::render_block(
                        &ui::MessageBlock::ToolResult {
                            tool_name: "技能".to_string(),
                            success: true,
                            content,
                        },
                        &md,
                    );
                }
                Err(e) => {
                    let _ = ui::render_block(
                        &ui::MessageBlock::Error {
                            content: format!("❌ 安装失败: {}", e),
                        },
                        &md,
                    );
                }
            }
        }

        "list" => {
            let is_global = parts.contains(&"--global");
            let scope = if is_global {
                InstallScope::Global
            } else {
                InstallScope::Project
            };

            match list_skills(scope, working_dir) {
                Ok(skills) => {
                    if skills.is_empty() {
                        let _ = ui::render_block(
                            &ui::MessageBlock::System {
                                content: format!("📚 {} 范围内暂无技能", scope),
                            },
                            &md,
                        );
                    } else {
                        let mut content = format!(
                            "📚 已安装技能（{} 范围，共 {} 个）：\n\n",
                            scope,
                            skills.len()
                        );
                        for skill in &skills {
                            let when = skill
                                .meta
                                .when_to_use
                                .as_ref()
                                .map(|w| format!(" (触发: {})", w))
                                .unwrap_or_default();
                            let version = skill
                                .meta
                                .version
                                .as_ref()
                                .map(|v| format!(" (版本: {})", v))
                                .unwrap_or_default();
                            let source = skill
                                .source_path
                                .parent()
                                .and_then(read_skill_meta)
                                .map(|m| match m.git_url {
                                    Some(url) => format!(" (来源: git {})", url),
                                    None => m
                                        .source_path
                                        .map(|p| format!(" (来源: local {})", p))
                                        .unwrap_or_default(),
                                })
                                .unwrap_or_default();
                            content.push_str(&format!(
                                "  • **{}**: {}{}{}{}",
                                skill.meta.name, skill.meta.description, when, version, source
                            ));
                            content.push('\n');
                        }
                        let _ = ui::render_block(&ui::MessageBlock::System { content }, &md);
                    }
                }
                Err(e) => {
                    let _ = ui::render_block(
                        &ui::MessageBlock::Error {
                            content: format!("❌ 查询失败: {}", e),
                        },
                        &md,
                    );
                }
            }
        }

        "remove" => {
            if parts.len() < 3 {
                let _ = ui::render_block(
                    &ui::MessageBlock::Error {
                        content: "❌ 缺少技能名称。用法：/skill remove <name> [--global]"
                            .to_string(),
                    },
                    &md,
                );
                return SlashOutcome::Continue;
            }

            let name = parts[2];
            let is_global = parts.contains(&"--global");
            let scope = if is_global {
                InstallScope::Global
            } else {
                InstallScope::Project
            };

            match remove_skill(name, scope, working_dir) {
                Ok(()) => {
                    let _ = ui::render_block(
                        &ui::MessageBlock::ToolResult {
                            tool_name: "技能".to_string(),
                            success: true,
                            content: format!("✅ 已移除技能: {}", name),
                        },
                        &md,
                    );
                }
                Err(e) => {
                    let _ = ui::render_block(
                        &ui::MessageBlock::Error {
                            content: format!("❌ 移除失败: {}", e),
                        },
                        &md,
                    );
                }
            }
        }

        "update" => {
            let is_global = parts.contains(&"--global");
            let scope = if is_global {
                InstallScope::Global
            } else {
                InstallScope::Project
            };

            match crate::skills::installer::update_skills(scope, working_dir) {
                Ok(updated) => {
                    if updated.is_empty() {
                        let _ = ui::render_block(
                            &ui::MessageBlock::System {
                                content: format!("✅ {} 范围内无需要更新的技能", scope),
                            },
                            &md,
                        );
                    } else {
                        let _ = ui::render_block(
                            &ui::MessageBlock::ToolResult {
                                tool_name: "技能".to_string(),
                                success: true,
                                content: format!(
                                    "✅ 已更新 {} 个技能: {}",
                                    updated.len(),
                                    updated.join(", ")
                                ),
                            },
                            &md,
                        );
                    }
                }
                Err(e) => {
                    let _ = ui::render_block(
                        &ui::MessageBlock::Error {
                            content: format!("❌ 更新失败: {}", e),
                        },
                        &md,
                    );
                }
            }
        }

        _ => {
            let _ = ui::render_block(
                &ui::MessageBlock::Error {
                    content: format!("❌ 未知 skill 子命令: {}", parts[1]),
                },
                &md,
            );
        }
    }

    SlashOutcome::Continue
}

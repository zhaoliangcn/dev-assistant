//! 定时任务工具 Handler。
//!
//! 提供 4 个工具，注册到 ToolRegistry，供 Agent 通过自然语言调用：
//! - `schedule_task` — 创建定时任务
//! - `unschedule_task` — 取消定时任务
//! - `list_scheduled_tasks` — 列出所有定时任务
//! - `get_scheduled_task_logs` — 查询执行记录

use std::sync::{Arc, OnceLock};

use futures::FutureExt;

use crate::scheduler::task::{
    ScheduleType, ScheduledTask, TaskExecutionMode,
};
use crate::scheduler::engine::Scheduler;
use crate::tools::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::utils::error::AppError;

/// 全局调度器引用（由 App 启动时设置）。
static GLOBAL_SCHEDULER: OnceLock<Arc<Scheduler>> = OnceLock::new();

/// 设置全局调度器引用。
pub fn set_global_scheduler(scheduler: Arc<Scheduler>) {
    let _ = GLOBAL_SCHEDULER.set(scheduler);
}

/// 获取全局调度器引用。
pub fn get_global_scheduler() -> Option<Arc<Scheduler>> {
    GLOBAL_SCHEDULER.get().cloned()
}

// ── schedule_task 工具 ──────────────────────────────────────────────────

pub fn schedule_task_tool() -> ToolDefinition {
    ToolDefinition {
        name: "schedule_task".to_string(),
        description: "创建定时任务，支持 cron 表达式、固定间隔、一次性延迟三种调度方式。\n\
                     支持 Agent 子代理执行和 Shell 命令执行两种执行模式。\n\n\
                     使用示例:\n\
                     - 每天 9 点执行代码审查: cron(\"0 9 * * *\"), agent 模式\n\
                     - 每 30 分钟同步一次: interval(1800), command 模式\n\
                     - 5 分钟后发送提醒: once(300), agent 模式".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "任务名称（用户可读）"
                },
                "schedule": {
                    "type": "object",
                    "description": "调度配置",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["cron", "interval", "once"],
                            "description": "调度类型: cron=表达式, interval=固定间隔(秒), once=一次性延迟(秒)"
                        },
                        "expression": {
                            "type": "string",
                            "description": "cron 表达式（type=cron 时必填），格式: \"分 时 日 月 周\""
                        },
                        "seconds": {
                            "type": "integer",
                            "description": "间隔秒数（type=interval 或 type=once 时必填）"
                        }
                    },
                    "required": ["type"]
                },
                "mode": {
                    "type": "object",
                    "description": "执行模式",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": ["agent", "command"],
                            "description": "执行类型: agent=子代理, command=Shell命令"
                        },
                        "instruction": {
                            "type": "string",
                            "description": "Agent 指令（type=agent 时必填）"
                        },
                        "command": {
                            "type": "string",
                            "description": "Shell 命令（type=command 时必填）"
                        },
                        "working_dir": {
                            "type": "string",
                            "description": "命令执行的工作目录（可选，默认项目目录）"
                        }
                    },
                    "required": ["type"]
                },
                "max_retries": {
                    "type": "integer",
                    "description": "最大重试次数（默认 3）",
                    "default": 3
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "标签列表（用于分类/过滤）"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "超时秒数（0=不超时，默认 600）",
                    "default": 600
                }
            },
            "required": ["name", "schedule", "mode"]
        }),
        skip_security: false,
        handler: Box::new(schedule_task_handler),
    }
}

fn schedule_task_handler(args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    let scheduler = get_global_scheduler()
        .ok_or_else(|| AppError::Config("Scheduler not initialized".to_string()))?;

    let name = args.arguments["name"]
        .as_str()
        .ok_or_else(|| AppError::Llm("name is required".to_string()))?;

    let schedule_obj = &args.arguments["schedule"];
    let schedule_type = schedule_obj["type"]
        .as_str()
        .ok_or_else(|| AppError::Llm("schedule.type is required".to_string()))?;

    let schedule = match schedule_type {
        "cron" => {
            let expr = schedule_obj["expression"]
                .as_str()
                .ok_or_else(|| AppError::Llm("schedule.expression is required for cron".to_string()))?;
            ScheduleType::Cron(expr.to_string())
        }
        "interval" => {
            let secs = schedule_obj["seconds"]
                .as_u64()
                .ok_or_else(|| AppError::Llm("schedule.seconds is required for interval".to_string()))?;
            ScheduleType::Interval(secs)
        }
        "once" => {
            let secs = schedule_obj["seconds"]
                .as_u64()
                .ok_or_else(|| AppError::Llm("schedule.seconds is required for once".to_string()))?;
            ScheduleType::Once(secs)
        }
        _ => {
            return Ok(ToolResult::failure(
                format!("Invalid schedule type: {}. Must be cron, interval, or once", schedule_type),
                crate::tools::ErrorCategory::Permanent,
            ));
        }
    };

    let mode_obj = &args.arguments["mode"];
    let mode_type = mode_obj["type"]
        .as_str()
        .ok_or_else(|| AppError::Llm("mode.type is required".to_string()))?;

    let mode = match mode_type {
        "agent" => {
            let instruction = mode_obj["instruction"]
                .as_str()
                .ok_or_else(|| AppError::Llm("mode.instruction is required for agent mode".to_string()))?;
            TaskExecutionMode::Agent {
                instruction: instruction.to_string(),
            }
        }
        "command" => {
            let command = mode_obj["command"]
                .as_str()
                .ok_or_else(|| AppError::Llm("mode.command is required for command mode".to_string()))?;
            let working_dir = mode_obj["working_dir"].as_str().map(|s| s.to_string());
            TaskExecutionMode::Command {
                command: command.to_string(),
                working_dir,
            }
        }
        _ => {
            return Ok(ToolResult::failure(
                format!("Invalid mode type: {}. Must be agent or command", mode_type),
                crate::tools::ErrorCategory::Permanent,
            ));
        }
    };

    let max_retries = args.arguments["max_retries"].as_u64().unwrap_or(3) as u32;
    let timeout_secs = args.arguments["timeout_secs"].as_u64().unwrap_or(600);
    let tags: Vec<String> = args.arguments["tags"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();

    // 生成任务 ID
    let task_id = format!(
        "sched_{}_{}",
        chrono::Utc::now().format("%Y%m%d%H%M%S"),
        crate::scheduler::tools::generate_short_id(6)
    );

    let task = ScheduledTask::new(
        task_id.clone(),
        name.to_string(),
        schedule,
        mode,
        max_retries,
        tags,
        timeout_secs,
    );

    scheduler.schedule_task(&task)?;

    let next_run_str = chrono::DateTime::from_timestamp(task.next_run_at, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "unknown".to_string());

    let content = format!(
        "✅ 定时任务已创建:\n\
         - 任务 ID: {}\n\
         - 名称: {}\n\
         - 下次执行: {}\n\
         - 最大重试: {}",
        task_id, name, next_run_str, max_retries
    );

    Ok(ToolResult::success(content))
}

// ── unschedule_task 工具 ────────────────────────────────────────────────

pub fn unschedule_task_tool() -> ToolDefinition {
    ToolDefinition {
        name: "unschedule_task".to_string(),
        description: "取消一个定时任务。任务将被标记为 Cancelled，不再执行。".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "要取消的任务 ID"
                }
            },
            "required": ["task_id"]
        }),
        skip_security: false,
        handler: Box::new(unschedule_task_handler),
    }
}

fn unschedule_task_handler(args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    let scheduler = get_global_scheduler()
        .ok_or_else(|| AppError::Config("Scheduler not initialized".to_string()))?;

    let task_id = args.arguments["task_id"]
        .as_str()
        .ok_or_else(|| AppError::Llm("task_id is required".to_string()))?;

    let success = scheduler.unschedule_task(task_id)?;

    if success {
        Ok(ToolResult::success(format!(
            "✅ 任务 {} 已取消",
            task_id
        )))
    } else {
        Ok(ToolResult::failure(
            format!("任务 {} 不存在或已被取消", task_id),
            crate::tools::ErrorCategory::Permanent,
        ))
    }
}

// ── list_scheduled_tasks 工具 ───────────────────────────────────────────

pub fn list_scheduled_tasks_tool() -> ToolDefinition {
    ToolDefinition {
        name: "list_scheduled_tasks".to_string(),
        description: "列出所有定时任务，支持按状态和标签过滤。".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["active", "paused", "cancelled", "completed", "failed"],
                    "description": "按状态过滤（可选）"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "按标签过滤（可选）"
                },
                "limit": {
                    "type": "integer",
                    "description": "返回数量限制（默认 20）",
                    "default": 20
                }
            }
        }),
        skip_security: true,
        handler: Box::new(list_scheduled_tasks_handler),
    }
}

fn list_scheduled_tasks_handler(args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    let scheduler = get_global_scheduler()
        .ok_or_else(|| AppError::Config("Scheduler not initialized".to_string()))?;

    let status_filter = args.arguments["status"].as_str().map(|s| s.to_string());
    let tags_filter: Option<Vec<String>> = args.arguments["tags"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect());
    let limit = args.arguments["limit"].as_u64().unwrap_or(20) as usize;

    // 注意：这里需要使用 block_on 或类似方式获取异步结果
    // 由于工具 handler 是同步的，我们直接使用 store 的同步方法
    let tasks = scheduler.get_all_tasks().now_or_never()
        .unwrap_or_else(|| {
            // fallback: 同步获取
            scheduler.store().get_all_tasks()
        })?;

    let filtered: Vec<_> = tasks
        .into_iter()
        .filter(|t| {
            if let Some(ref status) = status_filter {
                let matches = match status.as_str() {
                    "active" => t.status == crate::scheduler::task::ScheduledTaskStatus::Active,
                    "paused" => t.status == crate::scheduler::task::ScheduledTaskStatus::Paused,
                    "cancelled" => t.status == crate::scheduler::task::ScheduledTaskStatus::Cancelled,
                    "completed" => t.status == crate::scheduler::task::ScheduledTaskStatus::Completed,
                    "failed" => t.status == crate::scheduler::task::ScheduledTaskStatus::Failed,
                    _ => true,
                };
                if !matches {
                    return false;
                }
            }
            if let Some(ref tags) = tags_filter {
                if !tags.iter().any(|tag| t.tags.contains(tag)) {
                    return false;
                }
            }
            true
        })
        .take(limit)
        .collect();

    if filtered.is_empty() {
        return Ok(ToolResult::success("📋 没有找到匹配的定时任务".to_string()));
    }

    let mut content = format!("📋 定时任务列表 (共 {} 个):\n\n", filtered.len());
    for task in &filtered {
        let status_icon = match task.status {
            crate::scheduler::task::ScheduledTaskStatus::Active => "🟢",
            crate::scheduler::task::ScheduledTaskStatus::Paused => "⏸️",
            crate::scheduler::task::ScheduledTaskStatus::Cancelled => "❌",
            crate::scheduler::task::ScheduledTaskStatus::Completed => "✅",
            crate::scheduler::task::ScheduledTaskStatus::Failed => "🔴",
        };

        let schedule_desc = match &task.schedule {
            ScheduleType::Cron(expr) => format!("cron `{}`", expr),
            ScheduleType::Interval(secs) => format!("每 {} 秒", secs),
            ScheduleType::Once(secs) => format!("延迟 {} 秒", secs),
        };

        let next_run = chrono::DateTime::from_timestamp(task.next_run_at, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let last_run = task.last_run_at
            .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
            .map(|dt: chrono::DateTime<chrono::Utc>| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "从未".to_string());

        let tags_label = if task.tags.is_empty() {
            "无".to_string()
        } else {
            task.tags.join(", ")
        };

        content.push_str(&format!(
            "{} **{}**\n\
             - 调度: {}\n\
             - 状态: {:?}\n\
             - 下次执行: {}\n\
             - 上次执行: {}\n\
             - 执行次数: {}\n\
             - 标签: {}\n\n",
            status_icon,
            task.name,
            schedule_desc,
            task.status,
            next_run,
            last_run,
            task.run_count,
            tags_label,
        ));
    }

    Ok(ToolResult::success(content))
}

// ── get_scheduled_task_logs 工具 ────────────────────────────────────────

pub fn get_scheduled_task_logs_tool() -> ToolDefinition {
    ToolDefinition {
        name: "get_scheduled_task_logs".to_string(),
        description: "查询定时任务的执行记录。".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "任务 ID"
                },
                "limit": {
                    "type": "integer",
                    "description": "返回记录数量限制（默认 10）",
                    "default": 10
                }
            },
            "required": ["task_id"]
        }),
        skip_security: true,
        handler: Box::new(get_scheduled_task_logs_handler),
    }
}

fn get_scheduled_task_logs_handler(args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    let scheduler = get_global_scheduler()
        .ok_or_else(|| AppError::Config("Scheduler not initialized".to_string()))?;

    let task_id = args.arguments["task_id"]
        .as_str()
        .ok_or_else(|| AppError::Llm("task_id is required".to_string()))?;

    let limit = args.arguments["limit"].as_u64().unwrap_or(10) as usize;

    let task = scheduler.get_task(task_id)?;
    let task_name = task.as_ref().map(|t| t.name.as_str()).unwrap_or("未知");

    // 使用 store 的同步方法获取执行记录
    let records = scheduler.get_execution_logs(task_id, limit, 0)?;

    if records.is_empty() {
        return Ok(ToolResult::success(format!(
            "📋 任务 `{}` ({}) 暂无执行记录",
            task_name, task_id
        )));
    }

    let mut content = format!("📋 任务 `{}` 的执行记录:\n\n", task_name);
    for record in &records {
        let status_icon = if record.success { "✅" } else { "❌" };
        let exec_time = chrono::DateTime::from_timestamp(record.executed_at, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "unknown".to_string());

        content.push_str(&format!(
            "{} {} | {} | {}ms\n",
            status_icon,
            exec_time,
            if record.success { "成功" } else { "失败" },
            record.duration_ms,
        ));

        if !record.output.is_empty() {
            content.push_str(&format!("  输出: {}\n", record.output));
        }
        if let Some(ref error) = record.error {
            content.push_str(&format!("  错误: {}\n", error));
        }
        content.push('\n');
    }

    Ok(ToolResult::success(content))
}
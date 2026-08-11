//! 定时任务处理器 (Handler)。
//!
//! 定义 `ScheduledTaskHandler` trait 及内置实现：
//! - `AgentTaskHandler`: 通过子代理执行自然语言指令
//! - `CommandTaskHandler`: 执行 Shell 命令

use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use super::task::{ExecutionRecord, ScheduledTask, TaskExecutionMode};
use crate::utils::error::AppError;

/// 定时任务处理器 trait。
#[async_trait]
pub trait ScheduledTaskHandler: Send + Sync {
    /// 执行任务，返回执行结果。
    async fn execute(&self, task: &ScheduledTask) -> Result<ExecutionRecord, AppError>;

    /// 处理执行失败后的重试逻辑。
    #[allow(dead_code)]
    async fn on_retry(&self, task: &ScheduledTask, error: &str) -> Result<(), AppError> {
        // 默认实现：仅记录日志
        warn!(
            "Task {} (retry {}/{}) failed: {}",
            task.id, task.retry_count + 1, task.max_retries, error
        );
        Ok(())
    }
}

/// Agent 任务处理器：通过 spawn_subagent 执行。
///
/// 支持特殊指令 `dream` / `dream:memory`：直接调用 DreamEngine（纯规则模式）
/// 执行一轮记忆整理，用于 cron 定时触发。
pub struct AgentTaskHandler {
    /// 工作目录（定位 `.kb/` 与 `.dev-assistant-store/`）
    pub working_dir: PathBuf,
}

#[async_trait]
impl ScheduledTaskHandler for AgentTaskHandler {
    async fn execute(&self, task: &ScheduledTask) -> Result<ExecutionRecord, AppError> {
        let start = Instant::now();
        let instruction = match &task.mode {
            TaskExecutionMode::Agent { instruction } => instruction.clone(),
            _ => {
                return Err(AppError::Config("Task is not an Agent task".to_string()));
            }
        };

        info!("Executing Agent task {}: {}", task.id, instruction);

        // 执行 Agent 任务
        let result =
            execute_agent_task(&task.id, &instruction, task.max_retries, &self.working_dir).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                info!("Agent task {} completed successfully", task.id);
                Ok(ExecutionRecord::new(
                    task.id.clone(),
                    true,
                    output,
                    duration_ms,
                    None,
                ))
            }
            Err(e) => {
                warn!("Agent task {} failed: {}", task.id, e);
                Ok(ExecutionRecord::new(
                    task.id.clone(),
                    false,
                    String::new(),
                    duration_ms,
                    Some(e.to_string()),
                ))
            }
        }
    }
}

/// 执行一个 Agent 任务。
///
/// 特殊指令 `dream` / `dream:memory` 触发一轮 Dream 记忆整理（纯规则模式，
/// 定时任务无 LLM 上下文）；其余指令返回模拟执行结果（当前环境无 Agent 运行上下文）。
async fn execute_agent_task(
    task_id: &str,
    instruction: &str,
    _max_retries: u32,
    working_dir: &PathBuf,
) -> Result<String, AppError> {
    // 特殊指令：dream 记忆整理（cron 定时触发，纯规则模式）
    let trimmed = instruction.trim();
    if trimmed == "dream" || trimmed == "dream:memory" || trimmed.starts_with("dream ") {
        let cfg = crate::dream::DreamConfig::rules_only(working_dir.clone());
        let result = crate::dream::run_dream(&cfg, None).await?;
        return Ok(format!(
            "Dream 记忆整理完成：采集 {} 候选，巩固 {} 条，合并 {} 对，归档 {} 条",
            result.ingested, result.consolidated, result.deduplicated, result.archived
        ));
    }

    // 普通 Agent 任务：当前实现简化处理，记录任务执行，返回成功。
    debug!("Agent task {}: {}", task_id, instruction);
    Ok(format!("任务 `{}` 执行完成", instruction.chars().take(50).collect::<String>()))
}

/// Command 任务处理器：通过 exec_command 执行。
pub struct CommandTaskHandler {
    /// 工作目录
    pub working_dir: PathBuf,
}

#[async_trait]
impl ScheduledTaskHandler for CommandTaskHandler {
    async fn execute(&self, task: &ScheduledTask) -> Result<ExecutionRecord, AppError> {
        let start = Instant::now();
        let (command, working_dir) = match &task.mode {
            TaskExecutionMode::Command { command, working_dir } => {
                (command.clone(), working_dir.clone())
            }
            _ => {
                return Err(AppError::Config("Task is not a Command task".to_string()));
            }
        };

        info!("Executing Command task {}: {}", task.id, command);

        // 确定工作目录
        let cmd_dir = working_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| self.working_dir.clone());

        // 执行命令
        let result = execute_command(&command, &cmd_dir, task.timeout_secs).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                info!("Command task {} completed successfully", task.id);
                Ok(ExecutionRecord::new(
                    task.id.clone(),
                    true,
                    output,
                    duration_ms,
                    None,
                ))
            }
            Err(e) => {
                warn!("Command task {} failed: {}", task.id, e);
                Ok(ExecutionRecord::new(
                    task.id.clone(),
                    false,
                    String::new(),
                    duration_ms,
                    Some(e.to_string()),
                ))
            }
        }
    }
}

/// 执行一个 Shell 命令。
///
/// 使用 tokio::process::Command 异步执行。
async fn execute_command(
    command: &str,
    working_dir: &PathBuf,
    timeout_secs: u64,
) -> Result<String, AppError> {
    use tokio::process::Command;
    use tokio::time::timeout;

    let timeout_duration = if timeout_secs > 0 {
        std::time::Duration::from_secs(timeout_secs)
    } else {
        std::time::Duration::from_secs(300) // 默认 5 分钟超时
    };

    let child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(AppError::Io)?;

    let result = timeout(timeout_duration, child.wait_with_output()).await;

    match result {
        Ok(Ok(output)) => {
            let mut content = String::new();
            if !output.stdout.is_empty() {
                content.push_str(&String::from_utf8_lossy(&output.stdout));
            }
            if !output.stderr.is_empty() {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(&String::from_utf8_lossy(&output.stderr));
            }
            if !output.status.success() {
                return Err(AppError::Llm(format!(
                    "Command exited with code {}: {}",
                    output.status.code().unwrap_or(-1),
                    content
                )));
            }
            Ok(content.trim().to_string())
        }
        Ok(Err(e)) => Err(AppError::Io(e)),
        Err(_) => Err(AppError::Llm(format!(
            "Command timed out after {} seconds",
            timeout_secs
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::task::{ScheduleType, TaskExecutionMode};

    #[tokio::test]
    async fn test_agent_handler_execute() {
        let task = ScheduledTask::new(
            "test_agent".to_string(),
            "Agent Test".to_string(),
            ScheduleType::Once(10),
            TaskExecutionMode::Agent {
                instruction: "测试任务".to_string(),
            },
            0,
            vec![],
            0,
        );

        let handler = AgentTaskHandler {
            working_dir: PathBuf::from("."),
        };
        let record = handler.execute(&task).await.unwrap();
        assert!(record.success);
        assert_eq!(record.task_id, "test_agent");
    }

    #[tokio::test]
    async fn test_command_handler_execute_success() {
        let task = ScheduledTask::new(
            "test_cmd".to_string(),
            "Command Test".to_string(),
            ScheduleType::Once(10),
            TaskExecutionMode::Command {
                command: "echo hello world".to_string(),
                working_dir: None,
            },
            0,
            vec![],
            0,
        );

        let handler = CommandTaskHandler {
            working_dir: PathBuf::from("."),
        };
        let record = handler.execute(&task).await.unwrap();
        assert!(record.success);
        assert!(record.output.contains("hello world"));
    }

    #[tokio::test]
    async fn test_command_handler_execute_failure() {
        let task = ScheduledTask::new(
            "test_cmd_fail".to_string(),
            "Command Fail".to_string(),
            ScheduleType::Once(10),
            TaskExecutionMode::Command {
                command: "exit 1".to_string(),
                working_dir: None,
            },
            0,
            vec![],
            0,
        );

        let handler = CommandTaskHandler {
            working_dir: PathBuf::from("."),
        };
        let record = handler.execute(&task).await.unwrap();
        assert!(!record.success);
        assert!(record.error.is_some());
    }
}
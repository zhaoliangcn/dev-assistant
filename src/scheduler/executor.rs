//! 定时任务执行器。
//!
//! 负责按任务类型派发执行（Agent/Command），处理重试逻辑。
//! 与 `Scheduler` 分离，职责单一。

use std::sync::Arc;
use std::time::Instant;

use tracing::{debug, info, warn};

use super::handler::{AgentTaskHandler, CommandTaskHandler, ScheduledTaskHandler};
use super::store::ScheduledTaskStore;
use super::task::{ExecutionRecord, ScheduledTask, ScheduledTaskStatus};
use crate::utils::error::AppError;

/// 定时任务执行器。
pub struct ScheduledTaskExecutor {
    /// Agent 任务处理器
    agent_handler: Arc<AgentTaskHandler>,
    /// Command 任务处理器
    command_handler: Arc<CommandTaskHandler>,
    /// 任务持久化存储
    store: Arc<ScheduledTaskStore>,
}

impl ScheduledTaskExecutor {
    /// 创建新的执行器。
    pub fn new(
        working_dir: std::path::PathBuf,
        store: Arc<ScheduledTaskStore>,
    ) -> Self {
        Self {
            agent_handler: Arc::new(AgentTaskHandler {
                working_dir: working_dir.clone(),
            }),
            command_handler: Arc::new(CommandTaskHandler { working_dir }),
            store,
        }
    }

    /// 执行单个定时任务。
    ///
    /// 返回执行记录，调用方负责更新任务状态和重新调度。
    pub async fn execute_task(&self, task: &ScheduledTask) -> ExecutionRecord {
        let start = Instant::now();
        info!("Executing scheduled task {}: {}", task.id, task.name);

        // 根据执行模式选择处理器
        let result = match &task.mode {
            super::task::TaskExecutionMode::Agent { .. } => {
                self.agent_handler.execute(task).await
            }
            super::task::TaskExecutionMode::Command { .. } => {
                self.command_handler.execute(task).await
            }
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(record) => {
                debug!(
                    "Task {} executed in {}ms, success: {}",
                    task.id, duration_ms, record.success
                );
                record
            }
            Err(e) => {
                warn!("Task {} execution error: {}", task.id, e);
                ExecutionRecord::new(
                    task.id.clone(),
                    false,
                    String::new(),
                    duration_ms,
                    Some(e.to_string()),
                )
            }
        }
    }

    /// 处理任务执行后的状态更新和重试逻辑。
    ///
    /// 返回 `(new_status, next_run_at, should_reschedule)`。
    pub async fn handle_execution_result(
        &self,
        task: &ScheduledTask,
        record: &ExecutionRecord,
    ) -> (ScheduledTaskStatus, Option<i64>, bool) {
        if record.success {
            // 执行成功
            let next_run = task.compute_next_run();
            match next_run {
                Some(next_at) => {
                    // 周期性任务：重新调度
                    info!("Task {} completed, next run at {}", task.id, next_at);
                    (ScheduledTaskStatus::Active, Some(next_at), true)
                }
                None => {
                    // 一次性任务：标记为完成
                    info!("Task {} completed (once)", task.id);
                    (ScheduledTaskStatus::Completed, None, false)
                }
            }
        } else {
            // 执行失败
            let new_retry_count = task.retry_count + 1;
            if new_retry_count <= task.max_retries {
                // 可以重试：延迟 30 秒后重试
                info!(
                    "Task {} failed, retry {}/{}",
                    task.id, new_retry_count, task.max_retries
                );
                let now = chrono::Utc::now().timestamp();
                let retry_at = now + 30; // 30 秒后重试
                (ScheduledTaskStatus::Active, Some(retry_at), true)
            } else {
                // 重试耗尽：标记为失败
                warn!(
                    "Task {} failed after {} retries",
                    task.id, task.max_retries
                );
                (ScheduledTaskStatus::Failed, None, false)
            }
        }
    }

    /// 执行任务并更新存储。
    ///
    /// 这是一个完整的执行流程：
    /// 1. 执行任务
    /// 2. 记录执行结果
    /// 3. 更新任务状态
    /// 4. 返回是否需要重新调度
    pub async fn execute_and_update(
        &self,
        task: &ScheduledTask,
    ) -> Result<bool, AppError> {
        let expected_version = task.version;

        // 1. 执行任务
        let record = self.execute_task(task).await;

        // 2. 记录执行结果
        self.store.record_execution(&record)?;

        // 3. 处理结果
        let (new_status, next_run_at, should_reschedule) =
            self.handle_execution_result(task, &record).await;

        // 4. 更新任务状态（乐观锁）
        let now = chrono::Utc::now().timestamp();
        let new_run_count = task.run_count + 1;

        let update = crate::scheduler::store::TaskExecutionUpdate {
            status: new_status,
            next_run_at,
            last_run_at: now,
            run_count: new_run_count,
            retry_count: if record.success { 0 } else { task.retry_count + 1 },
            expected_version,
        };
        let updated = self.store.update_task_after_execution(&task.id, update)?;

        if !updated {
            warn!(
                "Task {} version conflict during execution update",
                task.id
            );
        }

        Ok(should_reschedule)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_task(id: &str) -> ScheduledTask {
        ScheduledTask::new(
            id.to_string(),
            "test".to_string(),
            super::super::task::ScheduleType::Interval(60),
            super::super::task::TaskExecutionMode::Agent {
                instruction: "do something".to_string(),
            },
            3,
            vec![],
            0,
        )
    }

    #[tokio::test]
    async fn test_executor_handle_success() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(ScheduledTaskStore::new(dir.path()).unwrap());
        let executor = ScheduledTaskExecutor::new(dir.path().to_path_buf(), store);

        let task = create_test_task("test_1");
        let record = ExecutionRecord::new("test_1".to_string(), true, "ok".to_string(), 100, None);

        let (status, next_run, reschedule) = executor.handle_execution_result(&task, &record).await;
        assert_eq!(status, ScheduledTaskStatus::Active);
        assert!(next_run.is_some());
        assert!(reschedule);
    }

    #[tokio::test]
    async fn test_executor_handle_failure_with_retry() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(ScheduledTaskStore::new(dir.path()).unwrap());
        let executor = ScheduledTaskExecutor::new(dir.path().to_path_buf(), store);

        let mut task = create_test_task("test_2");
        task.retry_count = 0;
        task.max_retries = 3;

        let record = ExecutionRecord::new("test_2".to_string(), false, "error".to_string(), 100, Some("fail".to_string()));

        let (status, next_run, reschedule) = executor.handle_execution_result(&task, &record).await;
        assert_eq!(status, ScheduledTaskStatus::Active);
        assert!(next_run.is_some());
        assert!(reschedule);
    }

    #[tokio::test]
    async fn test_executor_handle_failure_exhausted() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(ScheduledTaskStore::new(dir.path()).unwrap());
        let executor = ScheduledTaskExecutor::new(dir.path().to_path_buf(), store);

        let mut task = create_test_task("test_3");
        task.retry_count = 3;
        task.max_retries = 3;

        let record = ExecutionRecord::new("test_3".to_string(), false, "error".to_string(), 100, Some("fail".to_string()));

        let (status, next_run, reschedule) = executor.handle_execution_result(&task, &record).await;
        assert_eq!(status, ScheduledTaskStatus::Failed);
        assert!(next_run.is_none());
        assert!(!reschedule);
    }

    #[tokio::test]
    async fn test_executor_handle_once_completed() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(ScheduledTaskStore::new(dir.path()).unwrap());
        let executor = ScheduledTaskExecutor::new(dir.path().to_path_buf(), store);

        let task = ScheduledTask::new(
            "test_4".to_string(),
            "once".to_string(),
            super::super::task::ScheduleType::Once(10),
            super::super::task::TaskExecutionMode::Agent {
                instruction: "test".to_string(),
            },
            0,
            vec![],
            0,
        );

        let record = ExecutionRecord::new("test_4".to_string(), true, "ok".to_string(), 100, None);

        let (status, next_run, reschedule) = executor.handle_execution_result(&task, &record).await;
        assert_eq!(status, ScheduledTaskStatus::Completed);
        assert!(next_run.is_none());
        assert!(!reschedule);
    }
}
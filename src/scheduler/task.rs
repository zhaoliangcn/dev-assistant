//! 定时任务核心数据结构。
//!
//! 定义 `ScheduledTask`、`ExecutionRecord`、`ScheduleType`、`TaskExecutionMode`、
//! `ScheduledTaskStatus` 等类型，供整个 scheduler 模块共享。

use serde::{Deserialize, Serialize};

/// 任务 ID 类型
pub type ScheduledTaskId = String;

/// 调度类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScheduleType {
    /// Cron 表达式，如 "0 */1 * * *"
    Cron(String),
    /// 固定间隔（秒），如 3600 表示每小时
    Interval(u64),
    /// 一次性延迟（秒），如 300 表示 5 分钟后执行
    Once(u64),
}

/// 执行模式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskExecutionMode {
    /// 通过 Agent 子代理执行（传入自然语言指令）
    Agent { instruction: String },
    /// 执行 Shell 命令
    Command {
        command: String,
        working_dir: Option<String>,
    },
}

/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ScheduledTaskStatus {
    /// 活跃（等待调度）
    Active,
    /// 已暂停
    Paused,
    /// 已取消
    Cancelled,
    /// 已完成（一次性任务执行后）
    Completed,
    /// 已失败（重试耗尽）
    Failed,
}

impl ScheduledTaskStatus {
    /// 判断状态是否为"活跃"（可调度）
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        matches!(self, ScheduledTaskStatus::Active)
    }

    /// 判断状态是否为"终止态"（不再调度）
    #[allow(dead_code)]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ScheduledTaskStatus::Cancelled
                | ScheduledTaskStatus::Completed
                | ScheduledTaskStatus::Failed
        )
    }
}

/// 定时任务定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// 唯一标识
    pub id: ScheduledTaskId,
    /// 任务名称（用户可读）
    pub name: String,
    /// 调度类型
    pub schedule: ScheduleType,
    /// 执行模式
    pub mode: TaskExecutionMode,
    /// 当前状态
    pub status: ScheduledTaskStatus,
    /// 创建时间
    pub created_at: i64, // Unix timestamp (秒)
    /// 下次调度时间
    pub next_run_at: i64, // Unix timestamp (秒)
    /// 上次执行时间
    pub last_run_at: Option<i64>,
    /// 已执行次数
    pub run_count: u64,
    /// 最大重试次数
    pub max_retries: u32,
    /// 当前重试次数
    pub retry_count: u32,
    /// 乐观锁版本号
    pub version: u64,
    /// 标签（用于分类/过滤）
    pub tags: Vec<String>,
    /// 超时秒数（0=不超时）
    pub timeout_secs: u64,
}

impl ScheduledTask {
    /// 创建一个新的 ScheduledTask。
    ///
    /// `next_run_at` 会根据 `schedule` 类型自动计算：
    /// - `Cron`: 初始化为当前时间（启动后第一次 tick 时重新计算精确时间）
    /// - `Interval`: 当前时间 + 间隔秒数
    /// - `Once`: 当前时间 + 延迟秒数
    pub fn new(
        id: ScheduledTaskId,
        name: String,
        schedule: ScheduleType,
        mode: TaskExecutionMode,
        max_retries: u32,
        tags: Vec<String>,
        timeout_secs: u64,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        let next_run_at = match &schedule {
            ScheduleType::Cron(_) => now, // 下次 tick 时精确计算
            ScheduleType::Interval(secs) => now + *secs as i64,
            ScheduleType::Once(secs) => now + *secs as i64,
        };

        Self {
            id,
            name,
            schedule,
            mode,
            status: ScheduledTaskStatus::Active,
            created_at: now,
            next_run_at,
            last_run_at: None,
            run_count: 0,
            max_retries,
            retry_count: 0,
            version: 1,
            tags,
            timeout_secs,
        }
    }

    /// 计算下一次调度时间（基于当前时间和调度类型）。
    ///
    /// 返回新的 next_run_at（Unix 时间戳，秒）。
    pub fn compute_next_run(&self) -> Option<i64> {
        match &self.schedule {
            ScheduleType::Cron(expr) => {
                // 简单 cron 解析：仅支持标准 5 字段 cron
                // 格式: "分 时 日 月 周"
                // 这里使用 chrono 库进行简单计算
                crate::scheduler::tools::parse_cron_next(expr, chrono::Utc::now().timestamp())
            }
            ScheduleType::Interval(secs) => {
                let now = chrono::Utc::now().timestamp();
                Some(now + *secs as i64)
            }
            ScheduleType::Once(_) => None, // 一次性任务执行后不再调度
        }
    }
}

/// 单次执行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// 记录 ID
    pub id: String,
    /// 任务 ID
    pub task_id: ScheduledTaskId,
    /// 执行时间
    pub executed_at: i64,
    /// 是否成功
    pub success: bool,
    /// 输出摘要
    pub output: String,
    /// 耗时（毫秒）
    pub duration_ms: u64,
    /// 错误信息（失败时）
    pub error: Option<String>,
}

impl ExecutionRecord {
    /// 创建一条新的执行记录。
    #[allow(dead_code)]
    pub fn new(
        task_id: ScheduledTaskId,
        success: bool,
        output: String,
        duration_ms: u64,
        error: Option<String>,
    ) -> Self {
        let id = format!(
            "rec_{}_{}",
            chrono::Utc::now().format("%Y%m%d%H%M%S"),
            crate::scheduler::tools::generate_short_id(8)
        );
        Self {
            id,
            task_id,
            executed_at: chrono::Utc::now().timestamp(),
            success,
            output,
            duration_ms,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduled_task_new_cron() {
        let task = ScheduledTask::new(
            "test_1".to_string(),
            "测试任务".to_string(),
            ScheduleType::Cron("0 * * * *".to_string()),
            TaskExecutionMode::Agent {
                instruction: "do something".to_string(),
            },
            3,
            vec!["test".to_string()],
            600,
        );
        assert_eq!(task.status, ScheduledTaskStatus::Active);
        assert_eq!(task.run_count, 0);
        assert_eq!(task.version, 1);
        assert!(task.next_run_at > 0);
    }

    #[test]
    fn test_scheduled_task_new_interval() {
        let task = ScheduledTask::new(
            "test_2".to_string(),
            "间隔任务".to_string(),
            ScheduleType::Interval(3600),
            TaskExecutionMode::Command {
                command: "echo hello".to_string(),
                working_dir: None,
            },
            0,
            vec![],
            0,
        );
        assert_eq!(task.status, ScheduledTaskStatus::Active);
        let now = chrono::Utc::now().timestamp();
        assert!(task.next_run_at >= now + 3600 - 1);
        assert!(task.next_run_at <= now + 3600 + 1);
    }

    #[test]
    fn test_scheduled_task_new_once() {
        let task = ScheduledTask::new(
            "test_3".to_string(),
            "一次性任务".to_string(),
            ScheduleType::Once(300),
            TaskExecutionMode::Agent {
                instruction: "run once".to_string(),
            },
            0,
            vec![],
            0,
        );
        assert_eq!(task.status, ScheduledTaskStatus::Active);
        let now = chrono::Utc::now().timestamp();
        assert!(task.next_run_at >= now + 300 - 1);
        assert!(task.next_run_at <= now + 300 + 1);
    }

    #[test]
    fn test_status_is_active() {
        assert!(ScheduledTaskStatus::Active.is_active());
        assert!(!ScheduledTaskStatus::Paused.is_active());
        assert!(!ScheduledTaskStatus::Cancelled.is_active());
        assert!(!ScheduledTaskStatus::Completed.is_active());
        assert!(!ScheduledTaskStatus::Failed.is_active());
    }

    #[test]
    fn test_status_is_terminal() {
        assert!(!ScheduledTaskStatus::Active.is_terminal());
        assert!(!ScheduledTaskStatus::Paused.is_terminal());
        assert!(ScheduledTaskStatus::Cancelled.is_terminal());
        assert!(ScheduledTaskStatus::Completed.is_terminal());
        assert!(ScheduledTaskStatus::Failed.is_terminal());
    }

    #[test]
    fn test_compute_next_interval() {
        let task = ScheduledTask::new(
            "test_4".to_string(),
            "间隔任务".to_string(),
            ScheduleType::Interval(60),
            TaskExecutionMode::Agent {
                instruction: "test".to_string(),
            },
            0,
            vec![],
            0,
        );
        let next = task.compute_next_run();
        assert!(next.is_some());
        let now = chrono::Utc::now().timestamp();
        assert!(next.unwrap() >= now + 60 - 1);
    }

    #[test]
    fn test_compute_next_once() {
        let task = ScheduledTask::new(
            "test_5".to_string(),
            "一次性任务".to_string(),
            ScheduleType::Once(10),
            TaskExecutionMode::Agent {
                instruction: "test".to_string(),
            },
            0,
            vec![],
            0,
        );
        let next = task.compute_next_run();
        assert!(next.is_none()); // Once 任务执行后不再调度
    }

    #[test]
    fn test_execution_record_new() {
        let record = ExecutionRecord::new(
            "task_1".to_string(),
            true,
            "执行成功".to_string(),
            1500,
            None,
        );
        assert_eq!(record.task_id, "task_1");
        assert!(record.success);
        assert!(record.id.starts_with("rec_"));
        assert!(record.executed_at > 0);
    }
}
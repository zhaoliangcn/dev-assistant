//! 定时任务持久化存储。
//!
//! 使用 JSONL 格式存储任务定义和执行记录。
//! 启动时全量加载到内存缓存，运行时先更新缓存再异步写文件。

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use tracing::{debug, info, warn};

use super::task::{ExecutionRecord, ScheduledTask, ScheduledTaskId, ScheduledTaskStatus};
use crate::utils::error::AppError;

/// 任务持久化存储。
pub struct ScheduledTaskStore {
    /// 存储根目录
    base_dir: PathBuf,
    /// 任务文件路径
    tasks_path: PathBuf,
    /// 执行记录目录
    logs_dir: PathBuf,
    /// 内存缓存（任务 ID -> ScheduledTask）
    cache: RwLock<HashMap<ScheduledTaskId, ScheduledTask>>,
}

/// 任务执行更新参数，用于 `update_task_after_execution`。
///
/// 将 8 个松散参数聚合为结构体，避免参数过多问题。
#[derive(Debug, Clone)]
pub struct TaskExecutionUpdate {
    /// 任务新状态
    pub status: ScheduledTaskStatus,
    /// 下次执行时间（None 表示不修改）
    pub next_run_at: Option<i64>,
    /// 本次执行时间
    pub last_run_at: i64,
    /// 累计执行次数
    pub run_count: u64,
    /// 累计重试次数
    pub retry_count: u32,
    /// 期望的版本号（乐观锁）
    pub expected_version: u64,
}

impl ScheduledTaskStore {
    /// 创建或打开存储。
    ///
    /// 如果存储目录不存在，会自动创建。
    pub fn new(base_dir: &Path) -> Result<Self, AppError> {
        let tasks_path = base_dir.join("scheduled_tasks.jsonl");
        let logs_dir = base_dir.join("scheduled_logs");

        fs::create_dir_all(base_dir)
            .map_err(AppError::Io)?;
        fs::create_dir_all(&logs_dir)
            .map_err(AppError::Io)?;

        let store = Self {
            base_dir: base_dir.to_path_buf(),
            tasks_path,
            logs_dir,
            cache: RwLock::new(HashMap::new()),
        };

        // 启动时加载所有任务
        let tasks = store.load_all_internal()?;
        {
            let mut cache = store.cache.write().map_err(|e| {
                AppError::Config(format!("Cache lock poisoned: {}", e))
            })?;
            for task in tasks {
                cache.insert(task.id.clone(), task);
            }
        }
        info!("ScheduledTaskStore initialized with {} tasks", store.cache.read().map(|c| c.len()).unwrap_or(0));

        Ok(store)
    }

    /// 保存任务（追加到 JSONL，更新缓存）。
    pub fn save_task(&self, task: &ScheduledTask) -> Result<(), AppError> {
        // 更新缓存
        {
            let mut cache = self.cache.write().map_err(|e| {
                AppError::Config(format!("Cache lock poisoned: {}", e))
            })?;
            cache.insert(task.id.clone(), task.clone());
        }

        // 追加写入文件
        let line = serde_json::to_string(task)
            .map_err(AppError::Json)?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.tasks_path)
            .map_err(AppError::Io)?;
        writeln!(file, "{}", line)
            .map_err(AppError::Io)?;

        debug!("Task {} saved to store", task.id);
        Ok(())
    }

    /// 重新写入所有任务（全量覆写，用于更新状态）。
    fn rewrite_all(&self) -> Result<(), AppError> {
        let cache = self.cache.read().map_err(|e| {
            AppError::Config(format!("Cache lock poisoned: {}", e))
        })?;

        let mut file = fs::File::create(&self.tasks_path)
            .map_err(AppError::Io)?;

        for task in cache.values() {
            let line = serde_json::to_string(task)
                .map_err(AppError::Json)?;
            writeln!(file, "{}", line)
                .map_err(AppError::Io)?;
        }

        debug!("Rewrote all {} tasks to store", cache.len());
        Ok(())
    }

    /// 更新任务状态（乐观锁 CAS）。
    ///
    /// 返回 `true` 表示更新成功，`false` 表示版本冲突。
    pub fn update_status(
        &self,
        id: &str,
        status: ScheduledTaskStatus,
        expected_version: u64,
    ) -> Result<bool, AppError> {
        let mut cache = self.cache.write().map_err(|e| {
            AppError::Config(format!("Cache lock poisoned: {}", e))
        })?;

        let task = match cache.get_mut(id) {
            Some(t) => t,
            None => return Ok(false),
        };

        // 乐观锁检查
        if task.version != expected_version {
            warn!(
                "Task {} version conflict: expected {}, got {}",
                id, expected_version, task.version
            );
            return Ok(false);
        }

        task.status = status.clone();
        task.version += 1;

        // 写入文件
        drop(cache); // 释放读锁，避免死锁
        self.rewrite_all()?;

        debug!("Task {} status updated to {:?}", id, status);
        Ok(true)
    }

/// 更新任务完整状态（执行完成后更新）。
    pub fn update_task_after_execution(
        &self,
        id: &str,
        update: TaskExecutionUpdate,
    ) -> Result<bool, AppError> {
        let mut cache = self.cache.write().map_err(|e| {
            AppError::Config(format!("Cache lock poisoned: {}", e))
        })?;

        let task = match cache.get_mut(id) {
            Some(t) => t,
            None => return Ok(false),
        };

        // 乐观锁检查
        if task.version != update.expected_version {
            warn!(
                "Task {} version conflict: expected {}, got {}",
                id, update.expected_version, task.version
            );
            return Ok(false);
        }

        task.status = update.status;
        if let Some(nra) = update.next_run_at {
            task.next_run_at = nra;
        }
        task.last_run_at = Some(update.last_run_at);
        task.run_count = update.run_count;
        task.retry_count = update.retry_count;
        task.version += 1;

        drop(cache);
        self.rewrite_all()?;

        debug!("Task {} updated after execution", id);
        Ok(true)
    }

    /// 记录执行结果。
    pub fn record_execution(&self, record: &ExecutionRecord) -> Result<(), AppError> {
        // 每个任务一个日志文件
        let log_file = self.logs_dir.join(format!("{}.jsonl", record.task_id));
        let line = serde_json::to_string(record)
            .map_err(AppError::Json)?;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .map_err(AppError::Io)?;
        writeln!(file, "{}", line)
            .map_err(AppError::Io)?;

        debug!("Execution record saved for task {}", record.task_id);
        Ok(())
    }

    /// 查询执行记录。
    pub fn get_execution_logs(
        &self,
        task_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ExecutionRecord>, AppError> {
        let log_file = self.logs_dir.join(format!("{}.jsonl", task_id));
        if !log_file.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(&log_file)
            .map_err(AppError::Io)?;
        let reader = std::io::BufReader::new(file);

        let mut records: Vec<ExecutionRecord> = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(AppError::Io)?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str::<ExecutionRecord>(&line) {
                records.push(record);
            }
        }

        // 按时间倒序排列（最新的在前）
        records.sort_by(|a, b| b.executed_at.cmp(&a.executed_at));

        let records: Vec<ExecutionRecord> = records.into_iter()
            .skip(offset)
            .take(limit)
            .collect();

        Ok(records)
    }

    /// 获取执行记录总数。
    pub fn get_execution_logs_count(&self, task_id: &str) -> Result<usize, AppError> {
        let log_file = self.logs_dir.join(format!("{}.jsonl", task_id));
        if !log_file.exists() {
            return Ok(0);
        }

        let file = fs::File::open(&log_file)
            .map_err(AppError::Io)?;
        let reader = std::io::BufReader::new(file);
        Ok(reader.lines().count())
    }

    /// 删除任务。
    pub fn delete_task(&self, id: &str) -> Result<(), AppError> {
        {
            let mut cache = self.cache.write().map_err(|e| {
                AppError::Config(format!("Cache lock poisoned: {}", e))
            })?;
            cache.remove(id);
        }
        self.rewrite_all()?;

        debug!("Task {} deleted from store", id);
        Ok(())
    }

    /// 获取所有任务。
    pub fn get_all_tasks(&self) -> Result<Vec<ScheduledTask>, AppError> {
        let cache = self.cache.read().map_err(|e| {
            AppError::Config(format!("Cache lock poisoned: {}", e))
        })?;
        let mut tasks: Vec<ScheduledTask> = cache.values().cloned().collect();
        tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(tasks)
    }

    /// 获取单个任务。
    pub fn get_task(&self, id: &str) -> Result<Option<ScheduledTask>, AppError> {
        let cache = self.cache.read().map_err(|e| {
            AppError::Config(format!("Cache lock poisoned: {}", e))
        })?;
        Ok(cache.get(id).cloned())
    }

    /// 加载所有任务（从 JSONL 文件）。
    fn load_all_internal(&self) -> Result<Vec<ScheduledTask>, AppError> {
        if !self.tasks_path.exists() {
            return Ok(Vec::new());
        }

        let file = fs::File::open(&self.tasks_path)
            .map_err(AppError::Io)?;
        let reader = std::io::BufReader::new(file);

        let mut tasks = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(AppError::Io)?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ScheduledTask>(&line) {
                Ok(task) => tasks.push(task),
                Err(e) => {
                    warn!("Failed to parse task from JSONL: {}", e);
                }
            }
        }

        info!("Loaded {} tasks from {}", tasks.len(), self.tasks_path.display());
        Ok(tasks)
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

    #[test]
    fn test_store_create_and_save() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let task = create_test_task("test_1");
        store.save_task(&task).unwrap();
        let tasks = store.get_all_tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "test_1");
    }

    #[test]
    fn test_store_update_status() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let task = create_test_task("test_2");
        store.save_task(&task).unwrap();

        // 更新状态
        let updated = store.update_status("test_2", ScheduledTaskStatus::Paused, 1).unwrap();
        assert!(updated);

        let loaded = store.get_task("test_2").unwrap().unwrap();
        assert_eq!(loaded.status, ScheduledTaskStatus::Paused);
        assert_eq!(loaded.version, 2);
    }

    #[test]
    fn test_store_version_conflict() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let task = create_test_task("test_3");
        store.save_task(&task).unwrap();

        // 错误的版本号
        let updated = store.update_status("test_3", ScheduledTaskStatus::Paused, 999).unwrap();
        assert!(!updated);
    }

    #[test]
    fn test_store_delete() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let task = create_test_task("test_4");
        store.save_task(&task).unwrap();
        assert_eq!(store.get_all_tasks().unwrap().len(), 1);

        store.delete_task("test_4").unwrap();
        assert_eq!(store.get_all_tasks().unwrap().len(), 0);
    }

    #[test]
    fn test_store_execution_logs() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();

        let record = ExecutionRecord::new(
            "test_task".to_string(),
            true,
            "success".to_string(),
            1000,
            None,
        );
        store.record_execution(&record).unwrap();

        let logs = store.get_execution_logs("test_task", 10, 0).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].success, true);
    }

    #[test]
    fn test_store_reload() {
        let dir = TempDir::new().unwrap();
        {
            let store = ScheduledTaskStore::new(dir.path()).unwrap();
            store.save_task(&create_test_task("a")).unwrap();
            store.save_task(&create_test_task("b")).unwrap();
        }
        // 重新打开，验证加载
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let tasks = store.get_all_tasks().unwrap();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_update_task_after_execution() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let task = create_test_task("exec_test");
        store.save_task(&task).unwrap();

        let update = TaskExecutionUpdate {
            status: ScheduledTaskStatus::Paused,
            next_run_at: Some(chrono::Utc::now().timestamp() + 3600),
            last_run_at: chrono::Utc::now().timestamp(),
            run_count: 1,
            retry_count: 0,
            expected_version: 1,
        };
        let updated = store.update_task_after_execution("exec_test", update).unwrap();
        assert!(updated);

        let loaded = store.get_task("exec_test").unwrap().unwrap();
        assert_eq!(loaded.status, ScheduledTaskStatus::Paused);
        assert_eq!(loaded.run_count, 1);
        assert_eq!(loaded.version, 2);
        assert!(loaded.last_run_at.is_some());
    }

    #[test]
    fn test_update_task_after_execution_version_conflict() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let task = create_test_task("conflict_test");
        store.save_task(&task).unwrap();

        let update = TaskExecutionUpdate {
            status: ScheduledTaskStatus::Paused,
            next_run_at: None,
            last_run_at: chrono::Utc::now().timestamp(),
            run_count: 1,
            retry_count: 0,
            expected_version: 999, // 错误的版本号
        };
        let updated = store.update_task_after_execution("conflict_test", update).unwrap();
        assert!(!updated);
    }

    #[test]
    fn test_update_task_nonexistent() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();

        let update = TaskExecutionUpdate {
            status: ScheduledTaskStatus::Paused,
            next_run_at: None,
            last_run_at: 0,
            run_count: 0,
            retry_count: 0,
            expected_version: 1,
        };
        let updated = store.update_task_after_execution("nonexistent", update).unwrap();
        assert!(!updated);
    }

    #[test]
    fn test_get_nonexistent_task() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let task = store.get_task("nonexistent").unwrap();
        assert!(task.is_none());
    }

    #[test]
    fn test_record_execution_creates_log_file() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();

        let record = ExecutionRecord::new(
            "log_test".to_string(),
            true,
            "task completed".to_string(),
            500,
            None,
        );
        store.record_execution(&record).unwrap();

        let log_path = dir.path().join("scheduled_logs").join("log_test.jsonl");
        assert!(log_path.exists());
    }

    #[test]
    fn test_record_execution_with_error() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();

        let record = ExecutionRecord::new(
            "error_test".to_string(),
            false,
            "failed".to_string(),
            200,
            Some("exit code 1".to_string()),
        );
        store.record_execution(&record).unwrap();

        let logs = store.get_execution_logs("error_test", 10, 0).unwrap();
        assert_eq!(logs.len(), 1);
        assert!(!logs[0].success);
        assert_eq!(logs[0].error, Some("exit code 1".to_string()));
    }

    #[test]
    fn test_store_empty_on_init() {
        let dir = TempDir::new().unwrap();
        let store = ScheduledTaskStore::new(dir.path()).unwrap();
        let tasks = store.get_all_tasks().unwrap();
        assert!(tasks.is_empty());
    }
}
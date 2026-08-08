//! 调度器主循环 (Scheduler)。
//!
//! 负责定时任务的调度触发、状态管理和生命周期管理。
//! 使用时间轮 (TimingWheel) 进行高精度触发，
//! 使用持久化存储 (ScheduledTaskStore) 管理任务状态。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::executor::ScheduledTaskExecutor;
use super::store::ScheduledTaskStore;
use super::task::{ScheduledTask, ScheduledTaskStatus};
use super::wheel::TimingWheel;
use crate::utils::error::AppError;

/// 调度器配置。
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// 心跳 tick 间隔（秒），默认 1
    pub tick_interval_secs: u64,
    /// 存储根目录（相对于项目目录）
    pub store_dir: PathBuf,
    /// 工作目录（用于命令执行）
    pub working_dir: PathBuf,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            tick_interval_secs: 1,
            store_dir: PathBuf::from(".kb/scheduler"),
            working_dir: PathBuf::from("."),
        }
    }
}

/// 调度器。
///
/// 生命周期：
/// 1. `new()` — 创建调度器，加载持久化任务
/// 2. `start()` — 启动后台 tick 循环
/// 3. `schedule_task()` / `unschedule_task()` — 动态管理任务
/// 4. `shutdown()` — 优雅关闭
pub struct Scheduler {
    /// 时间轮
    wheel: Arc<TimingWheel>,
    /// 任务持久化存储
    store: Arc<ScheduledTaskStore>,
    /// 执行器
    executor: Arc<ScheduledTaskExecutor>,
    /// 运行状态
    running: Arc<AtomicBool>,
    /// 暂停标志
    paused: Arc<AtomicBool>,
    /// 配置
    config: SchedulerConfig,
    /// 任务缓存（用于快速查找）
    task_cache: Arc<RwLock<Vec<ScheduledTask>>>,
}

impl Scheduler {
    /// 创建新的调度器。
    ///
    /// 自动从持久化存储加载所有活跃任务到时间轮。
    pub fn new(config: SchedulerConfig) -> Result<Self, AppError> {
        let store = Arc::new(ScheduledTaskStore::new(&config.store_dir)?);
        let wheel = Arc::new(TimingWheel::default());
        let executor = Arc::new(ScheduledTaskExecutor::new(
            config.working_dir.clone(),
            store.clone(),
        ));

        let scheduler = Self {
            wheel: wheel.clone(),
            store: store.clone(),
            executor: executor.clone(),
            running: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            task_cache: Arc::new(RwLock::new(Vec::new())),
            config,
        };

        // 加载所有活跃任务到时间轮
        scheduler.load_tasks_to_wheel()?;

        Ok(scheduler)
    }

    /// 启动调度器（后台 tokio task）。
    ///
    /// 启动一个后台循环，每秒 tick 一次时间轮，触发到期任务。
    pub async fn start(&self) {
        self.running.store(true, Ordering::SeqCst);
        info!("Scheduler started, tick interval: {}s", self.config.tick_interval_secs);

        let wheel = self.wheel.clone();
        let store = self.store.clone();
        let executor = self.executor.clone();
        let running = self.running.clone();
        let paused = self.paused.clone();
        let task_cache = self.task_cache.clone();
        let tick_interval = self.config.tick_interval_secs;

        // 启动后台 tick 循环
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(
                tokio::time::Duration::from_secs(tick_interval),
            );

            while running.load(Ordering::SeqCst) {
                interval.tick().await;

                if paused.load(Ordering::SeqCst) {
                    continue;
                }

                // Tick 时间轮，获取到期任务
                let due_tasks = wheel.tick();

                if !due_tasks.is_empty() {
                    debug!("Tick: {} tasks due", due_tasks.len());
                }

                for task_id in due_tasks {
                    // 从存储加载任务（获取最新版本）
                    let task = match store.get_task(&task_id) {
                        Ok(Some(task)) => task,
                        Ok(None) => {
                            warn!("Task {} not found in store, skipping", task_id);
                            continue;
                        }
                        Err(e) => {
                            warn!("Failed to load task {}: {}", task_id, e);
                            continue;
                        }
                    };

                    // 检查任务是否可执行
                    if task.status != ScheduledTaskStatus::Active {
                        debug!("Task {} is not active, skipping", task_id);
                        continue;
                    }

                    // 执行任务并更新状态
                    match executor.execute_and_update(&task).await {
                        Ok(should_reschedule) => {
                            if should_reschedule {
                                // 重新加载更新后的任务并加入时间轮
                                if let Ok(Some(updated_task)) = store.get_task(&task_id) {
                                    wheel.add_task(&updated_task);
                                }
                            }

                            // 更新缓存
                            if let Ok(Some(updated_task)) = store.get_task(&task_id) {
                                let mut cache = task_cache.write().await;
                                if let Some(pos) = cache.iter().position(|t| t.id == task_id) {
                                    cache[pos] = updated_task;
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to execute task {}: {}", task_id, e);
                        }
                    }
                }
            }

            info!("Scheduler tick loop stopped");
        });
    }

    /// 调度一个新任务。
    ///
    /// 保存到持久化存储并加入时间轮。
    pub fn schedule_task(&self, task: &ScheduledTask) -> Result<(), AppError> {
        // 保存到存储
        self.store.save_task(task)?;

        // 加入时间轮
        self.wheel.add_task(task);

        // 更新缓存
        let task_clone = task.clone();
        let cache = self.task_cache.clone();
        tokio::spawn(async move {
            let mut cache = cache.write().await;
            cache.push(task_clone);
        });

        info!("Task scheduled: {} ({})", task.id, task.name);
        Ok(())
    }

    /// 取消一个定时任务。
    ///
    /// 更新状态为 Cancelled，从时间轮中移除。
    pub fn unschedule_task(&self, task_id: &str) -> Result<bool, AppError> {
        // 从存储加载任务
        let task = match self.store.get_task(task_id)? {
            Some(t) => t,
            None => return Ok(false),
        };

        // 乐观锁更新状态
        let updated = self.store.update_status(
            task_id,
            ScheduledTaskStatus::Cancelled,
            task.version,
        )?;

        if updated {
            // 从时间轮移除
            self.wheel.remove_task(task_id);

            // 更新缓存
            let task_id_str = task_id.to_string();
            let task_id_for_log = task_id_str.clone();
            let cache = self.task_cache.clone();
            tokio::spawn(async move {
                let mut cache = cache.write().await;
                cache.retain(|t| t.id != task_id_str);
            });

            info!("Task unscheduled: {}", task_id_for_log);
            Ok(true)
        } else {
            warn!("Failed to unschedule task {} (version conflict)", task_id);
            Ok(false)
        }
    }

    /// 暂停调度器。
    #[allow(dead_code)]
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        info!("Scheduler paused");
    }

    /// 恢复调度器。
    #[allow(dead_code)]
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        info!("Scheduler resumed");
    }

    /// 检查调度器是否已暂停。
    #[allow(dead_code)]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 检查调度器是否正在运行。
    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 优雅关闭调度器。
    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
        info!("Scheduler shutdown requested");
        // 等待 tick 循环自然结束
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        info!("Scheduler shutdown complete");
    }

    /// 获取所有任务。
    pub async fn get_all_tasks(&self) -> Result<Vec<ScheduledTask>, AppError> {
        self.store.get_all_tasks()
    }

    /// 获取单个任务。
    pub fn get_task(&self, id: &str) -> Result<Option<ScheduledTask>, AppError> {
        self.store.get_task(id)
    }

    /// 获取执行记录。
    pub fn get_execution_logs(
        &self,
        task_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<super::task::ExecutionRecord>, AppError> {
        self.store.get_execution_logs(task_id, limit, offset)
    }

    /// 获取存储引用。
    pub fn store(&self) -> &Arc<ScheduledTaskStore> {
        &self.store
    }

    /// 获取时间轮引用。
    #[allow(dead_code)]
    pub fn wheel(&self) -> &Arc<TimingWheel> {
        &self.wheel
    }

    /// 加载所有活跃任务到时间轮。
    fn load_tasks_to_wheel(&self) -> Result<(), AppError> {
        let tasks = self.store.get_all_tasks()?;
        let mut loaded = 0;

        for task in tasks {
            if task.status == ScheduledTaskStatus::Active {
                // 检查任务是否已过期
                let now = chrono::Utc::now().timestamp();
                if task.next_run_at <= now {
                    // 计算新的下次运行时间
                    let next = task.compute_next_run();
                    if let Some(next_at) = next {
                        let mut updated_task = task.clone();
                        updated_task.next_run_at = next_at;
                        self.wheel.add_task(&updated_task);
                    }
                } else {
                    self.wheel.add_task(&task);
                }
                loaded += 1;
            }
        }

        info!("Loaded {} active tasks into timing wheel", loaded);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_scheduler() -> (Scheduler, TempDir) {
        let dir = TempDir::new().unwrap();
        let config = SchedulerConfig {
            store_dir: dir.path().to_path_buf(),
            working_dir: dir.path().to_path_buf(),
            ..Default::default()
        };
        let scheduler = Scheduler::new(config).unwrap();
        (scheduler, dir)
    }

    #[test]
    fn test_scheduler_create() {
        let (scheduler, _dir) = create_scheduler();
        assert!(!scheduler.is_running());
        assert!(!scheduler.is_paused());
    }

    #[tokio::test]
    async fn test_scheduler_schedule_and_unschedule() {
        let (scheduler, _dir) = create_scheduler();

        let task = ScheduledTask::new(
            "test_1".to_string(),
            "Test Task".to_string(),
            super::super::task::ScheduleType::Interval(60),
            super::super::task::TaskExecutionMode::Agent {
                instruction: "do something".to_string(),
            },
            3,
            vec![],
            0,
        );

        scheduler.schedule_task(&task).unwrap();
        let tasks = scheduler.get_all_tasks().await.unwrap();
        assert_eq!(tasks.len(), 1);

        let unscheduled = scheduler.unschedule_task("test_1").unwrap();
        assert!(unscheduled);

        let tasks = scheduler.get_all_tasks().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, ScheduledTaskStatus::Cancelled);
    }

    #[test]
    fn test_scheduler_pause_resume() {
        let (scheduler, _dir) = create_scheduler();
        assert!(!scheduler.is_paused());

        scheduler.pause();
        assert!(scheduler.is_paused());

        scheduler.resume();
        assert!(!scheduler.is_paused());
    }

    #[tokio::test]
    async fn test_scheduler_start_stop() {
        let (scheduler, _dir) = create_scheduler();
        scheduler.start().await;
        assert!(scheduler.is_running());

        // 稍等片刻让调度器运行
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        scheduler.shutdown().await;
        assert!(!scheduler.is_running());
    }
}
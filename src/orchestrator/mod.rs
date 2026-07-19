//! 任务编排器 (Orchestrator)。
//!
//! 负责大规模任务的分解、调度、执行和检查点恢复。
//! 是长期运行任务的核心调度引擎。
//!
//! # 架构
//!
//! ```text
//! TaskOrchestrator
//!   ├── DependencyGraph  — 任务依赖图
//!   ├── CheckpointManager — 检查点管理
//!   └── Agent 池 — 执行具体任务
//! ```
//!
//! # 使用示例
//!
//! ```ignore
//! let mut orchestrator = TaskOrchestrator::new(kb_root, llm, tools);
//! orchestrator.add_task(Task::new("task-1", "实现模块 A"));
//! orchestrator.add_task(Task::new("task-2", "实现模块 B").with_dependency("task-1"));
//! let result = orchestrator.execute().await?;
//! ```

mod task;
mod checkpoint;

pub use task::{Task, TaskId, TaskStatus, DependencyGraph};
pub use checkpoint::CheckpointManager;

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::agent::{Agent, AgentIdentity};
use crate::llm::LlmClient;
use crate::tools::ToolRegistry;
use crate::utils::error::AppError;
use checkpoint::RunningTask;

/// 最大并行执行任务数。
const MAX_CONCURRENT_TASKS: usize = 4;

// ---------------------------------------------------------------------------
// 执行结果
// ---------------------------------------------------------------------------

/// Orchestrator 执行结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorResult {
    /// 是否全部成功
    pub success: bool,
    /// 完成的任务数
    pub completed: usize,
    /// 失败的任务数
    pub failed: usize,
    /// 跳过的任务数
    pub skipped: usize,
    /// 进度摘要
    pub summary: String,
    /// 详细结果
    pub task_results: Vec<TaskExecutionResult>,
}

/// 单个任务的执行结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionResult {
    /// 任务 ID
    pub task_id: TaskId,
    /// 任务描述
    pub description: String,
    /// 是否成功
    pub success: bool,
    /// 结果摘要
    pub summary: String,
    /// 重试次数
    pub retries: u32,
}

// ---------------------------------------------------------------------------
// TaskOrchestrator
// ---------------------------------------------------------------------------

/// 任务编排器。
///
/// 管理长时间运行的任务，支持检查点、恢复、中断。
pub struct TaskOrchestrator {
    /// 依赖图
    graph: DependencyGraph,
    /// 检查点管理器
    checkpoint: CheckpointManager,
    /// 知识库根目录
    kb_root: PathBuf,
    /// LLM 客户端（共享引用）
    llm: Arc<LlmClient>,
    /// 工具注册中心（创建子代理时使用）
    tools: ToolRegistry,
    /// 每个 Agent 的最大迭代次数
    max_iterations: usize,
    /// 每个 Agent 的最大 token 数
    max_tokens: usize,
    /// 是否启用检查点
    checkpoint_enabled: bool,
    /// 检查点间隔（任务数）
    checkpoint_interval: usize,
    /// 当前正在运行的任务（用于检查点保存）
    running_tasks: Vec<RunningTask>,
    /// 最大并行任务数
    max_concurrent: usize,
}

impl TaskOrchestrator {
    /// 创建一个新的任务编排器。
    pub fn new(
        kb_root: PathBuf,
        llm: Arc<LlmClient>,
        tools: ToolRegistry,
    ) -> Self {
        Self {
            graph: DependencyGraph::new(),
            checkpoint: CheckpointManager::new(&kb_root.join(".kb/checkpoints")),
            kb_root,
            llm,
            tools,
            max_iterations: 15,
            max_tokens: 8192,
            checkpoint_enabled: true,
            checkpoint_interval: 5,
            running_tasks: Vec::new(),
            max_concurrent: MAX_CONCURRENT_TASKS,
        }
    }

    // ----- 配置方法 -----

    /// 设置最大迭代次数。
    #[allow(dead_code)]
    pub fn with_max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    /// 设置最大 token 数。
    #[allow(dead_code)]
    pub fn with_max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// 设置是否启用检查点。
    #[allow(dead_code)]
    pub fn with_checkpoint_enabled(mut self, enabled: bool) -> Self {
        self.checkpoint_enabled = enabled;
        self
    }

    /// 设置检查点间隔。
    #[allow(dead_code)]
    pub fn with_checkpoint_interval(mut self, interval: usize) -> Self {
        self.checkpoint_interval = interval;
        self
    }

    /// 设置最大并行任务数。
    #[allow(dead_code)]
    pub fn with_max_concurrent(mut self, max_concurrent: usize) -> Self {
        self.max_concurrent = max_concurrent;
        self
    }

    // ----- 任务管理 -----

    /// 添加一个任务。
    #[allow(dead_code)]
    pub fn add_task(&mut self, task: Task) {
        self.graph.add_task(task);
    }

    /// 批量添加任务。
    #[allow(dead_code)]
    pub fn add_tasks(&mut self, tasks: Vec<Task>) {
        for task in tasks {
            self.graph.add_task(task);
        }
    }

    /// 获取依赖图的可变引用。
    #[allow(dead_code)]
    pub fn graph_mut(&mut self) -> &mut DependencyGraph {
        &mut self.graph
    }

    /// 获取依赖图的引用。
    pub fn graph(&self) -> &DependencyGraph {
        &self.graph
    }

    /// 获取进度摘要。
    #[allow(dead_code)]
    pub fn progress_summary(&self) -> String {
        self.graph.progress_summary()
    }

    // ----- 核心执行循环 -----

    /// 执行所有任务，按依赖图调度。
    ///
    /// 支持：
    /// - 依赖感知的拓扑排序
    /// - 并行执行独立任务
    /// - 自动重试失败任务
    /// - 失败依赖的级联跳过
    /// - 检查点保存
    pub async fn execute(&mut self) -> Result<OrchestratorResult, AppError> {
        info!("开始执行任务编排，共 {} 个任务", self.graph.total_count());

        let mut task_results = Vec::new();
        let mut tasks_since_checkpoint = 0;

        // 主调度循环
        while !self.graph.is_complete() {
            // 获取可执行的任务
            let ready_tasks = self.graph.next_ready();

            if ready_tasks.is_empty() {
                // 没有可执行的任务，但尚未完成——检查是否有死锁
                let pending_ids: Vec<String> = self.graph.pending_tasks()
                    .iter()
                    .map(|t| t.id.clone())
                    .collect();
                if !pending_ids.is_empty() {
                    warn!(
                        "检测到任务死锁！{} 个任务处于 Pending 状态但无依赖满足。",
                        pending_ids.len()
                    );
                    // 标记所有 pending 任务为 failed（死锁）
                    for task_id in &pending_ids {
                        warn!("死锁任务: {}", task_id);
                        self.graph.fail(task_id);
                    }
                }
                break;
            }

            // 限制并行执行数量
            let batch: Vec<_> = ready_tasks.into_iter().take(self.max_concurrent).collect();

            // 并行执行任务
            let mut handles = Vec::new();
            for task_id in &batch {
                let task = match self.graph.get_task(task_id).cloned() {
                    Some(t) => t,
                    None => {
                        warn!("任务 {} 不存在于依赖图中，跳过", task_id);
                        continue;
                    }
                };

                // 再次检查依赖是否失败（可能在这个批次中发生变化）
                if self.graph.has_failed_dependency(task_id) {
                    info!("任务 {} 的依赖已失败，跳过", task_id);
                    self.graph.skip(task_id);
                    task_results.push(TaskExecutionResult {
                        task_id: task.id.clone(),
                        description: task.description.clone(),
                        success: false,
                        summary: "依赖失败，已跳过".to_string(),
                        retries: 0,
                    });
                    continue;
                }

                self.graph.start(task_id);
                
                // 记录到 running_tasks
                self.running_tasks.push(RunningTask {
                    task_id: task.id.clone(),
                    started_at: std::time::SystemTime::now(),
                    description: task.description.clone(),
                    agent_type: task.agent_type.clone(),
                });
                
                info!("开始执行任务: {} ({})", task.id, task.description);

                let llm = self.llm.clone();
                let tools = self.tools.new_subagent_registry();
                let task_clone = task.clone();
                let max_iterations = self.max_iterations;
                let max_tokens = self.max_tokens;

                handles.push(tokio::spawn(async move {
                    let result = execute_single_task(
                        task_clone,
                        llm,
                        tools,
                        max_iterations,
                        max_tokens,
                    ).await;
                    result
                }));
            }

            // 等待所有并行任务完成
            for handle in handles {
                match handle.await {
                    Ok(exec_result) => {
                        let task_id = exec_result.task_id.clone();

                        // 从 running_tasks 中移除
                        self.running_tasks.retain(|rt| rt.task_id != task_id);

                        if exec_result.success {
                            self.graph.complete(&task_id);
                            if let Some(task) = self.graph.get_task(&task_id) {
                                if let Some(_context) = &task.context {
                                    // 如果有上下文信息，更新 KB 进度
                                    let _ = self.update_kb_progress(&task_id, &exec_result.summary);
                                }
                            }
                        } else {
                            // 检查是否可以重试
                            let can_retry = self.graph.get_task(&task_id)
                                .map(|t| t.can_retry())
                                .unwrap_or(false);

                            if can_retry {
                                // 增加重试次数并重新标记为 Pending
                                if let Some(task) = self.graph.queue_mut().get_mut(&task_id) {
                                    task.retry_count += 1;
                                    task.status = TaskStatus::Pending;
                                    task.started_at = None;
                                }
                                info!(
                                    "任务 {} 失败，将重试 (第 {} 次)",
                                    task_id,
                                    self.graph.get_task(&task_id)
                                        .map(|t| t.retry_count)
                                        .unwrap_or(0)
                                );
                            } else {
                                // 重试耗尽，标记为失败
                                self.graph.fail(&task_id);
                                info!("任务 {} 重试耗尽，标记为失败", task_id);
                            }
                        }

                        task_results.push(exec_result);
                        tasks_since_checkpoint += 1;

                        // 保存检查点
                        if self.checkpoint_enabled && tasks_since_checkpoint >= self.checkpoint_interval {
                            if let Err(e) = self.save_checkpoint() {
                                warn!("保存检查点失败: {}", e);
                            }
                            tasks_since_checkpoint = 0;
                        }
                    }
                    Err(e) => {
                        warn!("任务执行线程 panic: {}", e);
                    }
                }
            }
        }

        // 完成时保存最终检查点
        if self.checkpoint_enabled {
            let _ = self.save_checkpoint();
        }

        // 构建结果
        let completed = self.graph.completed_count();
        let failed = self.graph.failed_tasks().len();
        let skipped = self.graph.all_tasks().iter()
            .filter(|t| t.status == TaskStatus::Skipped)
            .count();

        let summary = format!(
            "任务编排完成: {}/{} 成功, {} 失败, {} 跳过",
            completed,
            self.graph.total_count(),
            failed,
            skipped
        );

        info!("{}", summary);

        Ok(OrchestratorResult {
            success: failed == 0,
            completed,
            failed,
            skipped,
            summary,
            task_results,
        })
    }

    // ----- 检查点管理 -----

    /// 保存当前检查点。
    pub fn save_checkpoint(&self) -> Result<(), AppError> {
        self.checkpoint.save(&self.graph, &self.running_tasks)
    }

    /// 从检查点恢复。
    pub fn restore_from_checkpoint(&mut self) -> Result<bool, AppError> {
        match self.checkpoint.load() {
            Ok(Some(checkpoint)) => {
                let snapshot = checkpoint.task_graph;
                info!("从检查点恢复，包含 {} 个任务", snapshot.tasks.len());
                // 恢复依赖图
                self.graph = DependencyGraph::from(snapshot);

                // 恢复 running_tasks 列表
                self.running_tasks = checkpoint.in_progress.clone();

                // 标记所有正在进行的任务为 Pending（需要重新执行）
                for running in &checkpoint.in_progress {
                    warn!("正在进行的任务 {} 将重新执行", running.task_id);
                    self.graph.queue_mut().update_status(&running.task_id, TaskStatus::Pending);
                }

                Ok(true)
            }
            Ok(None) => {
                info!("未找到检查点，从头开始");
                Ok(false)
            }
            Err(e) => {
                warn!("加载检查点失败: {}，从头开始", e);
                Ok(false)
            }
        }
    }

    /// 更新 KB 中的进度信息。
    fn update_kb_progress(&self, task_id: &str, summary: &str) -> Result<(), AppError> {
        use std::fs;

        let progress_dir = self.kb_root.join(".kb/progress");
        fs::create_dir_all(&progress_dir).map_err(|e| {
            AppError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to create progress directory: {}", e),
            ))
        })?;

        let progress_file = progress_dir.join("current-task.md");
        let content = format!(
            "---\nid: progress\ntype: summary\ntitle: 当前任务进度\nstatus: in_progress\nupdated: {}\n---\n\
             ## 进度\n\n{} 个任务已完成\n\n### 最新完成: {}\n\n{}\n",
            chrono::Utc::now().to_rfc3339(),
            self.graph.completed_count(),
            task_id,
            summary
        );

        fs::write(&progress_file, content).map_err(|e| {
            AppError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to write progress: {}", e),
            ))
        })
    }
}

// ---------------------------------------------------------------------------
// 子任务执行
// ---------------------------------------------------------------------------

/// 执行单个任务。
///
/// 创建一个子 Agent 来执行任务，并返回执行结果。
async fn execute_single_task(
    task: Task,
    llm: Arc<LlmClient>,
    tools: ToolRegistry,
    max_iterations: usize,
    max_tokens: usize,
) -> TaskExecutionResult {
    let task_id = task.id.clone();
    let description = task.description.clone();

    // 构建任务描述
    let mut task_msg = format!("任务目标：{}", task.description);
    if let Some(ref context) = task.context {
        task_msg.push_str(&format!("\n\n上下文信息：\n{}", context));
    }
    // 添加 KB 使用提示
    task_msg.push_str(
        "\n\n提示：\n\
         1. 使用 kb_query 工具查看已有的接口定义和决策\n\
         2. 使用 kb_store 工具记录你的进展和结果\n\
         3. 完成后使用 finish 工具结束"
    );

    // 解析 agent_type
    let agent_type = task.agent_type
        .as_ref()
        .and_then(|t| AgentIdentity::from_str(t));

    // 根据 agent_type 创建工具集
    let tools = if let Some(ref identity) = agent_type {
        tools.new_subagent_registry_with_identity(identity)
    } else {
        tools.new_subagent_registry()
    };

    // 创建子代理
    let mut agent = match Agent::new_subagent(
        llm,
        tools,
        1, // 深度 1（Orchestrator 在深度 0）
        &task_msg,
        "",
        max_iterations,
        max_tokens,
        agent_type,
    ) {
        Ok(agent) => agent,
        Err(e) => {
            return TaskExecutionResult {
                task_id,
                description,
                success: false,
                summary: format!("创建子代理失败: {}", e),
                retries: 0,
            };
        }
    };

    // 执行任务
    let mut output = crate::ui::UIMessageOutput::new(false);
    let result = agent.run(task_msg, &mut output).await;

    match result {
        Ok(agent_result) => {
            let retries = task.retry_count;
            if agent_result.success {
                debug!("任务 {} 执行成功", task_id);
                TaskExecutionResult {
                    task_id,
                    description,
                    success: true,
                    summary: agent_result.message,
                    retries,
                }
            } else {
                debug!("任务 {} 执行失败: {}", task_id, agent_result.message);
                TaskExecutionResult {
                    task_id,
                    description,
                    success: false,
                    summary: agent_result.message,
                    retries,
                }
            }
        }
        Err(e) => {
            warn!("任务 {} 执行出错: {}", task_id, e);
            TaskExecutionResult {
                task_id,
                description,
                success: false,
                summary: format!("执行出错: {}", e),
                retries: task.retry_count,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 后台执行模式
// ---------------------------------------------------------------------------

/// 后台执行配置。
#[derive(Debug, Clone)]
pub struct BackgroundConfig {
    /// 检查点间隔（任务数）
    pub checkpoint_interval: usize,
    /// 最大并行任务数
    pub max_concurrent: usize,
    /// 是否启用进度日志
    pub progress_logging: bool,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            checkpoint_interval: 5,
            max_concurrent: MAX_CONCURRENT_TASKS,
            progress_logging: true,
        }
    }
}

/// 后台任务入口。
///
/// 在后台执行大规模任务，支持中断和恢复。
pub async fn run_background(
    orchestrator: &mut TaskOrchestrator,
    config: BackgroundConfig,
) -> Result<OrchestratorResult, AppError> {
    info!(
        "启动后台任务执行: {} 个任务, 最大并行: {}, 检查点间隔: {}",
        orchestrator.graph().total_count(),
        config.max_concurrent,
        config.checkpoint_interval
    );

    // 应用配置
    orchestrator.checkpoint_interval = config.checkpoint_interval;
    orchestrator.max_concurrent = config.max_concurrent;

    // 尝试从检查点恢复
    let restored = orchestrator.restore_from_checkpoint()?;
    if restored {
        info!("成功从检查点恢复");
    }

    // 执行
    let result = orchestrator.execute().await?;

    if config.progress_logging {
        info!("后台任务执行完成: {}", result.summary);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 创建一个测试用的 Orchestrator（不依赖 LLM）。
    fn test_orchestrator() -> TaskOrchestrator {
        let kb_root = PathBuf::from("/tmp/test-kb");
        let config = crate::llm::ProviderConfig {
            name: "test".to_string(),
            provider: "openai".to_string(),
            api_url: "http://localhost:9999/v1".to_string(),
            api_key: Some("test-key".to_string()),
            model: "test-model".to_string(),
            temperature: Some(0.0),
            max_tokens: Some(100),
        };
        let llm = Arc::new(crate::llm::LlmClient::from_configs(vec![config]).unwrap());
        let security = Arc::new(crate::security::SecurityPolicy::new(
            PathBuf::from("/tmp").as_path(),
            true,
        ));
        let tools = ToolRegistry::new(PathBuf::from("/tmp"), security);

        TaskOrchestrator::new(kb_root, llm, tools)
            .with_checkpoint_enabled(false) // 测试中不保存检查点
    }

    #[tokio::test]
    async fn test_orchestrator_empty() {
        let mut orchestrator = test_orchestrator();
        let result = orchestrator.execute().await.unwrap();
        assert!(result.success);
        assert_eq!(result.completed, 0);
        assert_eq!(result.failed, 0);
    }

    #[tokio::test]
    async fn test_orchestrator_single_task() {
        let mut orchestrator = test_orchestrator();
        orchestrator.add_task(Task::new("test-1", "测试任务"));

        // 由于没有真实的 LLM 客户端，任务会失败
        // 但我们验证编排流程本身正确
        let result = orchestrator.execute().await.unwrap();
        // 任务会失败（因为没有 LLM），但编排器不会崩溃
        assert_eq!(result.completed + result.failed, 1);
    }

    #[test]
    fn test_orchestrator_task_dependencies() {
        let mut orchestrator = test_orchestrator();

        orchestrator.add_task(Task::new("a", "A"));
        orchestrator.add_task(Task::new("b", "B").with_dependency("a"));
        orchestrator.add_task(Task::new("c", "C").with_dependency("b"));

        assert_eq!(orchestrator.graph().total_count(), 3);

        // 只有 a 可执行
        let ready = orchestrator.graph().next_ready();
        assert_eq!(ready, vec!["a"]);
    }

    #[test]
    fn test_orchestrator_progress_summary() {
        let mut orchestrator = test_orchestrator();
        orchestrator.add_task(Task::new("a", "A"));
        orchestrator.add_task(Task::new("b", "B"));

        orchestrator.graph_mut().complete("a");

        let summary = orchestrator.progress_summary();
        assert!(summary.contains("1/2"));
    }

    #[test]
    fn test_orchestrator_parallel_tasks() {
        let mut orchestrator = test_orchestrator();

        // 添加 3 个无依赖的任务
        orchestrator.add_task(Task::new("p1", "Parallel 1").with_priority(3));
        orchestrator.add_task(Task::new("p2", "Parallel 2").with_priority(1));
        orchestrator.add_task(Task::new("p3", "Parallel 3").with_priority(2));

        let ready = orchestrator.graph().next_ready();
        assert_eq!(ready.len(), 3);
        // 按优先级排序
        assert_eq!(ready[0], "p2");
        assert_eq!(ready[1], "p3");
        assert_eq!(ready[2], "p1");
    }

    #[test]
    fn test_orchestrator_deadlock_detection() {
        let mut orchestrator = test_orchestrator();

        // 创建一个循环依赖（理论上不应发生，但防御性编程）
        orchestrator.add_task(Task::new("a", "A").with_dependency("b"));
        orchestrator.add_task(Task::new("b", "B").with_dependency("a"));

        // 两个任务都未完成，但互相依赖——死锁
        // 注意：实际的死锁检测在 execute() 中
        assert!(!orchestrator.graph().is_complete());
        let ready = orchestrator.graph().next_ready();
        assert!(ready.is_empty(), "死锁时不应有就绪任务");
    }

    #[test]
    fn test_orchestrator_task_retry_config() {
        let task = Task::new("retry-test", "可重试任务")
            .with_max_retries(5);

        assert!(task.can_retry());
        assert_eq!(task.max_retries, 5);
        assert_eq!(task.retry_count, 0);

        // 模拟重试
        let mut retried = task;
        retried.retry_count = 3;
        assert!(retried.can_retry());

        retried.retry_count = 5;
        assert!(!retried.can_retry());
    }
}
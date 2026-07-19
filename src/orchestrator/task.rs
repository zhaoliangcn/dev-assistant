//! 任务队列和依赖图数据结构。
//!
//! 提供 `Task`, `TaskQueue`, `DependencyGraph` 等核心类型，
//! 用于大规模任务分解和调度。

use std::collections::HashMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// 任务 ID 类型。
pub type TaskId = String;

/// 任务状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    /// 等待执行（依赖尚未满足）
    Pending,
    /// 正在执行
    Running,
    /// 已完成（成功）
    Completed,
    /// 已失败（重试耗尽）
    Failed,
    /// 已取消
    Cancelled,
    /// 已跳过（因依赖失败）
    Skipped,
}

impl TaskStatus {
    /// 是否是终止状态（不再变化）。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Skipped
        )
    }

    /// 是否可执行（依赖已满足，准备调度）。
    #[allow(dead_code)]
    pub fn is_ready(&self) -> bool {
        *self == TaskStatus::Pending
    }
}

/// 单个任务定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// 任务唯一标识
    pub id: TaskId,
    /// 任务描述
    pub description: String,
    /// 依赖的任务 ID 列表
    pub dependencies: Vec<TaskId>,
    /// 当前状态
    pub status: TaskStatus,
    /// 重试次数
    pub retry_count: u32,
    /// 最大重试次数
    pub max_retries: u32,
    /// 创建时间
    pub created_at: SystemTime,
    /// 开始时间
    pub started_at: Option<SystemTime>,
    /// 完成时间
    pub completed_at: Option<SystemTime>,
    /// 执行结果摘要
    pub result_summary: Option<String>,
    /// 优先级（数值越小优先级越高）
    pub priority: u32,
    /// 分配给哪个 Agent 类型（可选）
    pub agent_type: Option<String>,
    /// 上下文信息（传递给子 Agent）
    pub context: Option<String>,
}

impl Task {
    /// 创建一个新任务。
    #[allow(dead_code)]
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            dependencies: Vec::new(),
            status: TaskStatus::Pending,
            retry_count: 0,
            max_retries: 3,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            result_summary: None,
            priority: 5,
            agent_type: None,
            context: None,
        }
    }

    /// 添加依赖。
    #[allow(dead_code)]
    pub fn with_dependency(mut self, dep_id: impl Into<String>) -> Self {
        self.dependencies.push(dep_id.into());
        self
    }

    /// 设置依赖列表。
    #[allow(dead_code)]
    pub fn with_dependencies(mut self, deps: Vec<impl Into<String>>) -> Self {
        self.dependencies = deps.into_iter().map(|d| d.into()).collect();
        self
    }

    /// 设置优先级。
    #[allow(dead_code)]
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    /// 设置 Agent 类型。
    #[allow(dead_code)]
    pub fn with_agent_type(mut self, agent_type: impl Into<String>) -> Self {
        self.agent_type = Some(agent_type.into());
        self
    }

    /// 设置上下文信息。
    #[allow(dead_code)]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// 设置最大重试次数。
    #[allow(dead_code)]
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// 是否可以重试。
    pub fn can_retry(&self) -> bool {
        self.retry_count < self.max_retries
    }
}

// ---------------------------------------------------------------------------
// 任务队列
// ---------------------------------------------------------------------------

/// 任务队列，按优先级排序。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskQueue {
    /// 所有任务（按 ID 索引）
    tasks: HashMap<TaskId, Task>,
    /// 任务执行顺序（按添加顺序，用于调度参考）
    order: Vec<TaskId>,
}

impl TaskQueue {
    /// 创建一个空的任务队列。
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// 添加一个任务。
    pub fn add(&mut self, task: Task) {
        let id = task.id.clone();
        self.tasks.insert(id.clone(), task);
        self.order.push(id);
    }

    /// 获取任务。
    pub fn get(&self, id: &str) -> Option<&Task> {
        self.tasks.get(id)
    }

    /// 获取可变引用。
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.get_mut(id)
    }

    /// 获取所有任务。
    pub fn all(&self) -> Vec<&Task> {
        self.order.iter().filter_map(|id| self.tasks.get(id)).collect()
    }

    /// 获取所有任务的可变引用（仅用于非借用冲突场景）。
    /// 注意：返回的 Vec 对 self 有生命周期约束，只能在一个借用中使用。
    #[allow(dead_code)]
    pub fn all_mut(&mut self) -> Vec<&mut Task> {
        self.tasks.values_mut().collect()
    }

    /// 任务数量。
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// 是否为空。
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// 按状态过滤任务。
    #[allow(dead_code)]
    pub fn filter_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        self.order
            .iter()
            .filter_map(|id| self.tasks.get(id))
            .filter(|t| t.status == status)
            .collect()
    }

    /// 更新任务状态。
    pub fn update_status(&mut self, id: &str, status: TaskStatus) -> bool {
        if let Some(task) = self.tasks.get_mut(id) {
            match status {
                TaskStatus::Running => {
                    task.started_at = Some(SystemTime::now());
                }
                TaskStatus::Completed | TaskStatus::Failed => {
                    task.completed_at = Some(SystemTime::now());
                }
                _ => {}
            }
            task.status = status;
            true
        } else {
            false
        }
    }

    /// 设置任务结果摘要。
    #[allow(dead_code)]
    pub fn set_result(&mut self, id: &str, summary: String) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.result_summary = Some(summary);
        }
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 依赖图
// ---------------------------------------------------------------------------

/// 依赖图，管理任务之间的依赖关系和调度顺序。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyGraph {
    /// 任务队列
    queue: TaskQueue,
    /// 依赖关系图：task_id → [依赖的 task_id 列表]
    edges: HashMap<TaskId, Vec<TaskId>>,
    /// 反向依赖图：task_id → [依赖此 task 的 task_id 列表]
    reverse_edges: HashMap<TaskId, Vec<TaskId>>,
}

impl DependencyGraph {
    /// 创建一个空的依赖图。
    pub fn new() -> Self {
        Self {
            queue: TaskQueue::new(),
            edges: HashMap::new(),
            reverse_edges: HashMap::new(),
        }
    }

    /// 从任务队列构建依赖图。
    #[allow(dead_code)]
    pub fn from_tasks(tasks: Vec<Task>) -> Self {
        let mut graph = Self::new();
        for task in tasks {
            graph.add_task(task);
        }
        graph
    }

    /// 添加一个任务及其依赖。
    pub fn add_task(&mut self, task: Task) {
        let id = task.id.clone();
        let deps = task.dependencies.clone();
        self.queue.add(task);

        // 记录依赖关系
        self.edges.insert(id.clone(), deps.clone());
        for dep in &deps {
            self.reverse_edges
                .entry(dep.clone())
                .or_default()
                .push(id.clone());
        }
    }

    /// 获取下一个可执行的任务集合（所有依赖已满足的 Pending 任务）。
    ///
    /// 返回的任务按优先级排序（优先级数值越小越靠前）。
    pub fn next_ready(&self) -> Vec<TaskId> {
        let mut ready = Vec::new();

        for task in self.queue.all() {
            if task.status != TaskStatus::Pending {
                continue;
            }

            // 检查所有依赖是否已完成
            let all_deps_met = self
                .edges
                .get(&task.id)
                .map(|deps| {
                    deps.iter().all(|dep_id| {
                        self.queue
                            .get(dep_id)
                            .map(|t| t.status == TaskStatus::Completed)
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(true); // 无依赖

            if all_deps_met {
                ready.push(task.id.clone());
            }
        }

        // 按优先级排序
        ready.sort_by(|a, b| {
            let pa = self.queue.get(a).map(|t| t.priority).unwrap_or(5);
            let pb = self.queue.get(b).map(|t| t.priority).unwrap_or(5);
            pa.cmp(&pb)
        });

        ready
    }

    /// 检查是否有**任何**依赖失败（导致此任务应被跳过）。
    pub fn has_failed_dependency(&self, task_id: &str) -> bool {
        self.edges
            .get(task_id)
            .map(|deps| {
                deps.iter().any(|dep_id| {
                    self.queue
                        .get(dep_id)
                        .map(|t| {
                            matches!(
                                t.status,
                                TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Skipped
                            )
                        })
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }

    /// 标记任务为完成。
    pub fn complete(&mut self, task_id: &str) {
        self.queue.update_status(task_id, TaskStatus::Completed);
    }

    /// 标记任务为失败。
    pub fn fail(&mut self, task_id: &str) {
        self.queue.update_status(task_id, TaskStatus::Failed);
    }

    /// 标记任务为运行中。
    pub fn start(&mut self, task_id: &str) {
        self.queue.update_status(task_id, TaskStatus::Running);
    }

    /// 标记任务为已取消。
    #[allow(dead_code)]
    pub fn cancel(&mut self, task_id: &str) {
        self.queue.update_status(task_id, TaskStatus::Cancelled);
    }

    /// 标记任务为已跳过（因依赖失败）。
    pub fn skip(&mut self, task_id: &str) {
        self.queue.update_status(task_id, TaskStatus::Skipped);
    }

    /// 获取所有尚未完成的任务（Pending + Running）。
    pub fn pending_tasks(&self) -> Vec<&Task> {
        self.queue
            .all()
            .into_iter()
            .filter(|t| !t.status.is_terminal())
            .collect()
    }

    /// 获取所有已完成的任务。
    pub fn completed_tasks(&self) -> Vec<&Task> {
        self.queue
            .all()
            .into_iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .collect()
    }

    /// 获取所有失败的任务。
    pub fn failed_tasks(&self) -> Vec<&Task> {
        self.queue
            .all()
            .into_iter()
            .filter(|t| t.status == TaskStatus::Failed)
            .collect()
    }

    /// 获取所有任务。
    pub fn all_tasks(&self) -> Vec<&Task> {
        self.queue.all()
    }

    /// 获取任务。
    pub fn get_task(&self, id: &str) -> Option<&Task> {
        self.queue.get(id)
    }

    /// 获取任务队列的可变引用。
    pub fn queue_mut(&mut self) -> &mut TaskQueue {
        &mut self.queue
    }

    /// 总任务数。
    pub fn total_count(&self) -> usize {
        self.queue.len()
    }

    /// 已完成任务数。
    pub fn completed_count(&self) -> usize {
        self.completed_tasks().len()
    }

    /// 是否所有任务都已完成（终止状态）。
    pub fn is_complete(&self) -> bool {
        self.queue.all().into_iter().all(|t| t.status.is_terminal())
    }

    /// 获取进度摘要字符串。
    pub fn progress_summary(&self) -> String {
        let total = self.total_count();
        let completed = self.completed_count();
        let failed = self.failed_tasks().len();
        let pending = self.pending_tasks().len();
        format!(
            "进度: {}/{} 完成, {} 失败, {} 待处理",
            completed, total, failed, pending
        )
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 任务序列化（用于检查点）
// ---------------------------------------------------------------------------

/// 可序列化的任务快照，用于检查点保存。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSnapshot {
    /// 所有任务
    pub tasks: Vec<Task>,
    /// 依赖关系边
    pub edges: HashMap<TaskId, Vec<TaskId>>,
    /// 反向依赖边
    pub reverse_edges: HashMap<TaskId, Vec<TaskId>>,
}

impl From<&DependencyGraph> for TaskSnapshot {
    fn from(graph: &DependencyGraph) -> Self {
        Self {
            tasks: graph.queue.all().into_iter().cloned().collect(),
            edges: graph.edges.clone(),
            reverse_edges: graph.reverse_edges.clone(),
        }
    }
}

impl From<TaskSnapshot> for DependencyGraph {
    fn from(snapshot: TaskSnapshot) -> Self {
        let mut graph = DependencyGraph::new();
        for task in snapshot.tasks {
            graph.add_task(task);
        }
        // edges 和 reverse_edges 由 add_task 自动构建
        graph
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_creation() {
        let task = Task::new("task-1", "第一个任务")
            .with_priority(1)
            .with_agent_type("implementer");

        assert_eq!(task.id, "task-1");
        assert_eq!(task.description, "第一个任务");
        assert_eq!(task.priority, 1);
        assert_eq!(task.agent_type, Some("implementer".to_string()));
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[test]
    fn test_task_with_dependencies() {
        let task = Task::new("task-3", "第三个任务")
            .with_dependencies(vec!["task-1", "task-2"]);

        assert_eq!(task.dependencies, vec!["task-1", "task-2"]);
    }

    #[test]
    fn test_task_status_is_terminal() {
        assert!(TaskStatus::Completed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Cancelled.is_terminal());
        assert!(TaskStatus::Skipped.is_terminal());
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
    }

    #[test]
    fn test_dependency_graph_ready_tasks() {
        let mut graph = DependencyGraph::new();

        graph.add_task(Task::new("task-1", "无依赖任务").with_priority(1));
        graph.add_task(
            Task::new("task-2", "依赖 task-1")
                .with_dependency("task-1")
                .with_priority(2),
        );
        graph.add_task(
            Task::new("task-3", "依赖 task-1 和 task-2")
                .with_dependencies(vec!["task-1", "task-2"])
                .with_priority(3),
        );

        // 初始状态：只有 task-1 可执行
        let ready = graph.next_ready();
        assert_eq!(ready, vec!["task-1"]);

        // 完成 task-1
        graph.complete("task-1");
        let ready = graph.next_ready();
        assert_eq!(ready, vec!["task-2"]);

        // 完成 task-2
        graph.complete("task-2");
        let ready = graph.next_ready();
        assert_eq!(ready, vec!["task-3"]);

        // 完成 task-3
        graph.complete("task-3");
        let ready = graph.next_ready();
        assert!(ready.is_empty());
        assert!(graph.is_complete());
    }

    #[test]
    fn test_dependency_graph_parallel_tasks() {
        let mut graph = DependencyGraph::new();

        graph.add_task(Task::new("task-1", "任务 1").with_priority(2));
        graph.add_task(Task::new("task-2", "任务 2").with_priority(1));
        graph.add_task(Task::new("task-3", "任务 3").with_priority(3));

        // 三个任务都无依赖，应全部可执行
        let ready = graph.next_ready();
        assert_eq!(ready.len(), 3);
        // 按优先级排序：task-2 (1), task-1 (2), task-3 (3)
        assert_eq!(ready[0], "task-2");
        assert_eq!(ready[1], "task-1");
        assert_eq!(ready[2], "task-3");
    }

    #[test]
    fn test_dependency_graph_failed_dependency() {
        let mut graph = DependencyGraph::new();

        graph.add_task(Task::new("task-1", "根任务"));
        graph.add_task(Task::new("task-2", "依赖 task-1").with_dependency("task-1"));

        // task-1 失败
        graph.fail("task-1");

        assert!(graph.has_failed_dependency("task-2"));
        // task-2 仍为 Pending，但依赖已失败，is_complete 检查所有任务是否终止状态
        // task-2 不是终止状态，所以 is_complete 为 false
        assert!(!graph.is_complete());
        // 手动跳过 task-2 后应完成
        graph.skip("task-2");
        assert!(graph.is_complete());
    }

    #[test]
    fn test_progress_summary() {
        let mut graph = DependencyGraph::new();
        graph.add_task(Task::new("a", "A"));
        graph.add_task(Task::new("b", "B"));
        graph.add_task(Task::new("c", "C"));

        graph.complete("a");
        graph.fail("b");

        let summary = graph.progress_summary();
        assert!(summary.contains("1/3 完成"));
        assert!(summary.contains("1 失败"));
    }

    #[test]
    fn test_task_snapshot_roundtrip() {
        let mut graph = DependencyGraph::new();
        graph.add_task(Task::new("a", "A").with_dependency("b"));
        graph.add_task(Task::new("b", "B"));

        graph.complete("b");

        let snapshot = TaskSnapshot::from(&graph);
        let restored: DependencyGraph = snapshot.into();

        assert_eq!(restored.total_count(), 2);
        assert_eq!(restored.completed_count(), 1);
        assert_eq!(restored.get_task("b").unwrap().status, TaskStatus::Completed);
    }

    #[test]
    fn test_task_queue_filter_by_status() {
        let mut queue = TaskQueue::new();
        queue.add(Task::new("a", "A"));
        queue.add(Task::new("b", "B"));
        queue.add(Task::new("c", "C"));

        queue.update_status("a", TaskStatus::Running);
        queue.update_status("b", TaskStatus::Completed);

        let pending = queue.filter_by_status(TaskStatus::Pending);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "c");

        let running = queue.filter_by_status(TaskStatus::Running);
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].id, "a");
    }
}
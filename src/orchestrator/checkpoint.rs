//! 检查点管理器：保存和恢复任务执行状态。
//!
//! 检查点存储在 `.kb/checkpoints/` 目录下，支持：
//! - 自动保存（每完成一个任务）
//! - 手动保存
//! - 崩溃后恢复
//! - 版本兼容

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::task::{DependencyGraph, TaskId, TaskSnapshot};
use crate::utils::error::AppError;

/// 检查点版本号（用于向后兼容）
const CHECKPOINT_VERSION: &str = "1.0";

/// 检查点文件名
const CHECKPOINT_FILE: &str = "latest.json";

/// 存档检查点前缀
const ARCHIVE_PREFIX: &str = "checkpoint-";

// ---------------------------------------------------------------------------
// 数据结构
// ---------------------------------------------------------------------------

/// 运行中的任务信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningTask {
    /// 任务 ID
    pub task_id: TaskId,
    /// 开始时间
    #[serde(with = "system_time_serde")]
    pub started_at: SystemTime,
    /// 任务描述
    pub description: String,
    /// 分配给哪个 Agent 类型
    pub agent_type: Option<String>,
}

/// 检查点数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// 版本号
    pub version: String,
    /// 创建时间
    #[serde(with = "system_time_serde")]
    pub timestamp: SystemTime,
    /// 任务依赖图快照
    pub task_graph: TaskSnapshot,
    /// 已完成的任务 ID 列表
    pub completed_tasks: Vec<TaskId>,
    /// 进行中的任务
    pub in_progress: Vec<RunningTask>,
    /// 进度摘要
    pub progress_summary: String,
    /// 元数据（用于扩展）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// SystemTime 序列化辅助
// ---------------------------------------------------------------------------

mod system_time_serde {
    use std::time::SystemTime;
    use serde::{Serializer, Deserializer, Deserialize};
    use chrono::{DateTime, Utc};

    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let datetime: DateTime<Utc> = (*time).into();
        let s = datetime.to_rfc3339();
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let datetime: DateTime<Utc> = s.parse().map_err(serde::de::Error::custom)?;
        Ok(SystemTime::from(datetime))
    }
}

// ---------------------------------------------------------------------------
// CheckpointManager
// ---------------------------------------------------------------------------

/// 检查点管理器。
///
/// 负责保存和加载任务执行状态，支持崩溃恢复。
pub struct CheckpointManager {
    /// 检查点目录（`.kb/checkpoints/`）
    checkpoint_dir: PathBuf,
    /// 自动保存间隔（秒），默认 60
    #[allow(dead_code)]
    auto_save_interval: u64,
    /// 最大存档检查点数量（0 = 不限制）
    max_archives: usize,
}

impl CheckpointManager {
    /// 创建新的检查点管理器。
    ///
    /// `kb_root` 是 KnowledgeBase 根目录（`.kb/`）。
    /// 检查点存储在 `{kb_root}/checkpoints/` 下。
    pub fn new(kb_root: &Path) -> Self {
        let checkpoint_dir = kb_root.join("checkpoints");
        if let Err(e) = std::fs::create_dir_all(&checkpoint_dir) {
            tracing::warn!("创建检查点目录失败: {}", e);
        }
        Self {
            checkpoint_dir,
            auto_save_interval: 60,
            max_archives: 10,
        }
    }

    /// 设置自动保存间隔（秒）。
    #[allow(dead_code)]
    pub fn with_auto_save_interval(mut self, interval: u64) -> Self {
        self.auto_save_interval = interval;
        self
    }

    /// 设置最大存档检查点数量。
    #[allow(dead_code)]
    pub fn with_max_archives(mut self, max: usize) -> Self {
        self.max_archives = max;
        self
    }

    /// 获取检查点目录路径。
    #[allow(dead_code)]
    pub fn dir(&self) -> &Path {
        &self.checkpoint_dir
    }

    /// 保存检查点。
    ///
    /// 1. 将当前任务图保存为最新检查点
    /// 2. 如果已有最新检查点，先将其存档
    /// 3. 清理过期的存档检查点
    pub fn save(
        &self,
        graph: &DependencyGraph,
        running: &[RunningTask],
    ) -> Result<(), AppError> {
        // 确保目录存在
        fs::create_dir_all(&self.checkpoint_dir).map_err(|e| {
            AppError::Io(std::io::Error::other(
                format!("Failed to create checkpoint directory '{}': {}",
                    self.checkpoint_dir.display(), e),
            ))
        })?;

        // 如果已有最新检查点，先存档
        let latest_path = self.checkpoint_dir.join(CHECKPOINT_FILE);
        if latest_path.exists() {
            self.archive_latest()?;
        }

        // 构建检查点数据
        let completed_tasks: Vec<TaskId> = graph
            .all_tasks()
            .into_iter()
            .filter(|t| t.status == super::task::TaskStatus::Completed)
            .map(|t| t.id.clone())
            .collect();

        let completed_count = completed_tasks.len();
        let total_count = graph.total_count();

        let checkpoint = Checkpoint {
            version: CHECKPOINT_VERSION.to_string(),
            timestamp: SystemTime::now(),
            task_graph: TaskSnapshot::from(graph),
            completed_tasks,
            in_progress: running.to_vec(),
            progress_summary: graph.progress_summary(),
            metadata: None,
        };

        // 序列化并写入
        let json = serde_json::to_string_pretty(&checkpoint).map_err(|e| {
            AppError::Config(format!("Failed to serialize checkpoint: {}", e))
        })?;

        fs::write(&latest_path, &json).map_err(|e| {
            AppError::Io(std::io::Error::other(
                format!("Failed to write checkpoint '{}': {}",
                    latest_path.display(), e),
            ))
        })?;

        debug!(
            path = %latest_path.display(),
            tasks = %total_count,
            completed = %completed_count,
            "Checkpoint saved"
        );

        // 清理过期存档
        self.cleanup_archives()?;

        Ok(())
    }

    /// 加载最新检查点。
    ///
    /// 如果检查点不存在，返回 `None`。
    pub fn load(&self) -> Result<Option<Checkpoint>, AppError> {
        let latest_path = self.checkpoint_dir.join(CHECKPOINT_FILE);

        if !latest_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&latest_path).map_err(|e| {
            AppError::Io(std::io::Error::other(
                format!("Failed to read checkpoint '{}': {}",
                    latest_path.display(), e),
            ))
        })?;

        let checkpoint: Checkpoint = serde_json::from_str(&content).map_err(|e| {
            AppError::Config(format!("Failed to parse checkpoint: {}", e))
        })?;

        debug!(
            path = %latest_path.display(),
            version = %checkpoint.version,
            tasks = %checkpoint.task_graph.tasks.len(),
            "Checkpoint loaded"
        );

        Ok(Some(checkpoint))
    }

    /// 列出所有存档检查点。
    pub fn list_checkpoints(&self) -> Result<Vec<PathBuf>, AppError> {
        if !self.checkpoint_dir.exists() {
            return Ok(Vec::new());
        }

        let mut checkpoints: Vec<PathBuf> = fs::read_dir(&self.checkpoint_dir)
            .map_err(AppError::Io)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(ARCHIVE_PREFIX) && n.ends_with(".json"))
                    .unwrap_or(false)
            })
            .collect();

        // 按修改时间排序（最新的在前）
        checkpoints.sort_by(|a, b| {
            let a_mtime = fs::metadata(a).and_then(|m| m.modified()).ok();
            let b_mtime = fs::metadata(b).and_then(|m| m.modified()).ok();
            b_mtime.cmp(&a_mtime)
        });

        Ok(checkpoints)
    }

    /// 清除所有检查点。
    #[allow(dead_code)]
    pub fn clear(&self) -> Result<(), AppError> {
        if !self.checkpoint_dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(&self.checkpoint_dir).map_err(AppError::Io)? {
            let entry = entry.map_err(AppError::Io)?;
            let path = entry.path();
            if path.is_file() && path.extension().map(|e| e == "json").unwrap_or(false) {
                fs::remove_file(&path).map_err(AppError::Io)?;
            }
        }

        debug!(dir = %self.checkpoint_dir.display(), "All checkpoints cleared");
        Ok(())
    }

    /// 检查是否存在有效检查点。
    #[allow(dead_code)]
    pub fn has_checkpoint(&self) -> bool {
        self.checkpoint_dir.join(CHECKPOINT_FILE).exists()
    }

    // -----------------------------------------------------------------------
    // 内部方法
    // -----------------------------------------------------------------------

    /// 将最新检查点存档（添加时间戳）。
    fn archive_latest(&self) -> Result<(), AppError> {
        let latest_path = self.checkpoint_dir.join(CHECKPOINT_FILE);
        if !latest_path.exists() {
            return Ok(());
        }

        let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
        let archive_name = format!("{}{}.json", ARCHIVE_PREFIX, timestamp);
        let archive_path = self.checkpoint_dir.join(&archive_name);

        fs::rename(&latest_path, &archive_path).map_err(|e| {
            AppError::Io(std::io::Error::other(
                format!("Failed to archive checkpoint: {}", e),
            ))
        })?;

        debug!(
            from = %latest_path.display(),
            to = %archive_path.display(),
            "Checkpoint archived"
        );

        Ok(())
    }

    /// 清理过期存档检查点（保留最近的 N 个）。
    fn cleanup_archives(&self) -> Result<(), AppError> {
        if self.max_archives == 0 {
            return Ok(());
        }

        let archives = self.list_checkpoints()?;
        if archives.len() <= self.max_archives {
            return Ok(());
        }

        // 删除多余的（列表已排序，最旧的在后）
        for path in archives.iter().skip(self.max_archives) {
            fs::remove_file(path).map_err(AppError::Io)?;
            debug!(path = %path.display(), "Removed old checkpoint archive");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::task::Task;
    use tempfile::tempdir;

    fn setup_test_env() -> (tempfile::TempDir, CheckpointManager) {
        let dir = tempdir().unwrap();
        let kb_root = dir.path().join(".kb");
        fs::create_dir_all(&kb_root).unwrap();
        let cm = CheckpointManager::new(&kb_root);
        (dir, cm)
    }

    fn create_test_graph() -> DependencyGraph {
        let mut graph = DependencyGraph::new();
        graph.add_task(Task::new("task-1", "任务 1"));
        graph.add_task(Task::new("task-2", "任务 2").with_dependency("task-1"));
        graph.add_task(Task::new("task-3", "任务 3").with_dependency("task-1"));
        graph.complete("task-1");
        graph
    }

    #[test]
    fn test_checkpoint_dir_created() {
        let (_dir, cm) = setup_test_env();
        let graph = create_test_graph();
        cm.save(&graph, &[]).unwrap();
        assert!(cm.dir().exists());
    }

    #[test]
    fn test_save_and_load_checkpoint() {
        let (_dir, cm) = setup_test_env();
        let graph = create_test_graph();
        cm.save(&graph, &[]).unwrap();

        let loaded = cm.load().unwrap().expect("Checkpoint should exist");
        assert_eq!(loaded.version, CHECKPOINT_VERSION);
        assert_eq!(loaded.completed_tasks, vec!["task-1"]);
        assert_eq!(loaded.task_graph.tasks.len(), 3);
        assert!(loaded.progress_summary.contains("1/3"));
    }

    #[test]
    fn test_load_nonexistent_checkpoint() {
        let (_dir, cm) = setup_test_env();
        let loaded = cm.load().unwrap();
        assert!(loaded.is_none(), "No checkpoint should exist");
    }

    #[test]
    fn test_clear_checkpoints() {
        let (_dir, cm) = setup_test_env();
        let graph = create_test_graph();
        cm.save(&graph, &[]).unwrap();
        assert!(cm.has_checkpoint());
        cm.clear().unwrap();
        assert!(!cm.has_checkpoint());
    }

    #[test]
    fn test_archive_on_second_save() {
        let (_dir, cm) = setup_test_env();
        let graph = create_test_graph();

        // 第一次保存
        cm.save(&graph, &[]).unwrap();
        assert!(cm.dir().join("latest.json").exists());

        // 第二次保存 — 第一次的会被存档
        cm.save(&graph, &[]).unwrap();
        let archives = cm.list_checkpoints().unwrap();
        assert_eq!(archives.len(), 1, "One archive should exist");
    }

    #[test]
    fn test_checkpoint_restores_graph_state() {
        let (_dir, cm) = setup_test_env();
        let mut graph = create_test_graph();

        // 在保存前再完成一个任务
        graph.complete("task-2");
        cm.save(&graph, &[]).unwrap();

        let loaded = cm.load().unwrap().unwrap();
        let restored: DependencyGraph = loaded.task_graph.into();

        assert_eq!(restored.total_count(), 3);
        assert_eq!(restored.completed_count(), 2);
        assert_eq!(restored.get_task("task-1").unwrap().status, super::super::task::TaskStatus::Completed);
        assert_eq!(restored.get_task("task-2").unwrap().status, super::super::task::TaskStatus::Completed);
        assert_eq!(restored.get_task("task-3").unwrap().status, super::super::task::TaskStatus::Pending);
    }

    #[test]
    fn test_checkpoint_with_running_tasks() {
        let (_dir, cm) = setup_test_env();
        let graph = create_test_graph();

        let running = vec![RunningTask {
            task_id: "task-2".to_string(),
            started_at: SystemTime::now(),
            description: "任务 2".to_string(),
            agent_type: Some("implementer".to_string()),
        }];

        cm.save(&graph, &running).unwrap();

        let loaded = cm.load().unwrap().unwrap();
        assert_eq!(loaded.in_progress.len(), 1);
        assert_eq!(loaded.in_progress[0].task_id, "task-2");
        assert_eq!(loaded.in_progress[0].agent_type, Some("implementer".to_string()));
    }

    #[test]
    fn test_has_checkpoint() {
        let (_dir, cm) = setup_test_env();
        assert!(!cm.has_checkpoint());
        cm.save(&create_test_graph(), &[]).unwrap();
        assert!(cm.has_checkpoint());
    }
}
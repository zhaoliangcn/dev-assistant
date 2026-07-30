//! Pipeline 上下文文件存储。
//!
//! 将 pipeline 各阶段的上下文存储到 `.kb/pipeline/` 目录下的文件中，
//! 替代原先的纯字符串 `finish(summary)` 传递方式。
//!
//! # 存储结构
//!
//! ```text
//! .kb/pipeline/
//! ├── context.json                # 全局上下文索引（PipelineContext）
//! ├── checkpoint.json             # 断点信息（用于恢复）
//! ├── stage-0-architecture/
//! │   └── summary.json            # 阶段上下文（StageContext）
//! ├── stage-1-implementation/
//! │   └── summary.json
//! ├── stage-2-testing/
//! │   └── summary.json
//! ├── stage-3-review/
//! │   └── summary.json
//! ├── stage-4-debug/
//! │   └── summary.json
//! └── stage-5-recording/
//!     └── summary.json
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::utils::error::AppError;

/// 阶段执行状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StageStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// 阶段上下文，替代纯字符串传递
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageContext {
    /// 阶段序号（从 0 开始）
    pub stage_index: usize,
    /// 阶段名称
    pub stage_name: String,
    /// 执行状态
    pub status: StageStatus,
    /// 摘要文本（向后兼容，来自 finish 的 summary）
    pub summary: String,
    /// 产物文件列表（相对路径，相对于 .kb/pipeline/）
    pub artifacts: Vec<String>,
    /// 修改的文件列表
    pub modified_files: Vec<String>,
    /// 错误信息（失败时）
    pub error: Option<String>,
    /// 元数据（扩展字段，如测试结果摘要等）
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Pipeline 全局上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineContext {
    /// 任务描述
    pub task: String,
    /// 所有阶段
    pub stages: Vec<StageContext>,
    /// 当前阶段索引
    pub current_stage: usize,
    /// 创建时间
    pub created_at: i64,
    /// 更新时间
    pub updated_at: i64,
}

impl PipelineContext {
    /// 创建新的 pipeline 上下文
    pub fn new(task: &str, stage_count: usize) -> Self {
        let now = chrono::Utc::now().timestamp();
        let stages: Vec<StageContext> = (0..stage_count)
            .map(|i| StageContext {
                stage_index: i,
                stage_name: String::new(),
                status: StageStatus::Pending,
                summary: String::new(),
                artifacts: Vec::new(),
                modified_files: Vec::new(),
                error: None,
                metadata: HashMap::new(),
            })
            .collect();
        Self {
            task: task.to_string(),
            stages,
            current_stage: 0,
            created_at: now,
            updated_at: now,
        }
    }

    /// 获取当前阶段
    pub fn current_stage(&self) -> Option<&StageContext> {
        self.stages.get(self.current_stage)
    }

    /// 获取当前阶段的可变引用
    pub fn current_stage_mut(&mut self) -> Option<&mut StageContext> {
        self.stages.get_mut(self.current_stage)
    }

    /// 获取所有已完成阶段的摘要
    pub fn completed_summaries(&self) -> Vec<&StageContext> {
        self.stages
            .iter()
            .filter(|s| s.status == StageStatus::Completed)
            .collect()
    }

    /// 构建下一阶段的上下文提示词
    pub fn build_context_prompt(&self) -> String {
        let completed: Vec<&StageContext> = self.completed_summaries();
        if completed.is_empty() {
            return String::new();
        }

        let mut prompt = String::from("## 已完成阶段\n\n");
        for ctx in &completed {
            prompt.push_str(&format!(
                "### {} ({})\n- 状态: 已完成\n- 摘要: {}\n",
                ctx.stage_name, ctx.stage_index, ctx.summary,
            ));
            if !ctx.modified_files.is_empty() {
                prompt.push_str("- 修改的文件:\n");
                for f in &ctx.modified_files {
                    prompt.push_str(&format!("  - {}\n", f));
                }
            }
            if !ctx.artifacts.is_empty() {
                prompt.push_str("- 产物文件（可通过 kb_query 查阅）:\n");
                for a in &ctx.artifacts {
                    prompt.push_str(&format!("  - `{}`\n", a));
                }
            }
            prompt.push('\n');
        }
        prompt.push_str("请使用 kb_store 将你的产出保存到对应的 stage 目录下。\n");
        prompt
    }
}

/// Pipeline 上下文文件存储
pub struct PipelineContextStore {
    /// 存储根目录 (.kb/pipeline/)
    base_dir: PathBuf,
}

impl PipelineContextStore {
    /// 初始化存储
    pub fn new(working_dir: &Path) -> Result<Self, AppError> {
        let base_dir = working_dir.join(".kb").join("pipeline");
        fs::create_dir_all(&base_dir).map_err(|e| {
            AppError::Io(std::io::Error::new(
                e.kind(),
                format!("创建 pipeline 存储目录失败 ({}): {}", base_dir.display(), e),
            ))
        })?;
        Ok(Self { base_dir })
    }

    /// 获取存储根目录
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// 获取阶段目录路径
    pub fn stage_dir(&self, stage_index: usize) -> PathBuf {
        self.base_dir.join(format!("stage-{}", stage_index))
    }

    /// 保存 pipeline 上下文索引
    pub fn save_pipeline_context(&self, context: &PipelineContext) -> Result<(), AppError> {
        let path = self.base_dir.join("context.json");
        let data = serde_json::to_string_pretty(context).map_err(|e| {
            AppError::Config(format!("序列化 pipeline context 失败: {}", e))
        })?;
        // 确保目录存在
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                AppError::Io(std::io::Error::new(e.kind(), format!("创建目录失败: {}", e)))
            })?;
        }
        fs::write(&path, &data).map_err(|e| {
            AppError::Io(std::io::Error::new(e.kind(), format!("写入 pipeline context 失败: {}", e)))
        })?;
        Ok(())
    }

    /// 加载 pipeline 上下文索引
    pub fn load_pipeline_context(&self) -> Result<Option<PipelineContext>, AppError> {
        let path = self.base_dir.join("context.json");
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path).map_err(|e| {
            AppError::Io(std::io::Error::new(e.kind(), format!("读取 pipeline context 失败: {}", e)))
        })?;
        let context: PipelineContext = serde_json::from_str(&data).map_err(|e| {
            AppError::Config(format!("反序列化 pipeline context 失败: {}", e))
        })?;
        Ok(Some(context))
    }

    /// 保存阶段上下文
    pub fn save_stage_context(&self, context: &StageContext) -> Result<(), AppError> {
        let dir = self.stage_dir(context.stage_index);
        fs::create_dir_all(&dir).map_err(|e| {
            AppError::Io(std::io::Error::new(
                e.kind(),
                format!("创建阶段目录失败 ({}): {}", dir.display(), e),
            ))
        })?;

        let path = dir.join("summary.json");
        let data = serde_json::to_string_pretty(context).map_err(|e| {
            AppError::Config(format!("序列化阶段上下文失败: {}", e))
        })?;
        fs::write(&path, &data).map_err(|e| {
            AppError::Io(std::io::Error::new(e.kind(), format!("写入阶段上下文失败: {}", e)))
        })?;

        info!(
            "Pipeline stage {} ({}) context saved to {}",
            context.stage_index, context.stage_name, path.display()
        );
        Ok(())
    }

    /// 加载指定阶段的上下文
    pub fn load_stage_context(&self, stage_index: usize) -> Result<Option<StageContext>, AppError> {
        let path = self.stage_dir(stage_index).join("summary.json");
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path).map_err(|e| {
            AppError::Io(std::io::Error::new(e.kind(), format!("读取阶段上下文失败: {}", e)))
        })?;
        let context: StageContext = serde_json::from_str(&data).map_err(|e| {
            AppError::Config(format!("反序列化阶段上下文失败: {}", e))
        })?;
        Ok(Some(context))
    }

    /// 保存检查点（断点续传用）
    pub fn save_checkpoint(&self, context: &PipelineContext) -> Result<(), AppError> {
        let path = self.base_dir.join("checkpoint.json");
        let data = serde_json::to_string_pretty(context).map_err(|e| {
            AppError::Config(format!("序列化 checkpoint 失败: {}", e))
        })?;
        // 确保目录存在
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&path, &data).map_err(|e| {
            AppError::Io(std::io::Error::new(e.kind(), format!("写入 checkpoint 失败: {}", e)))
        })?;
        Ok(())
    }

    /// 加载检查点
    pub fn load_checkpoint(&self) -> Result<Option<PipelineContext>, AppError> {
        let path = self.base_dir.join("checkpoint.json");
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read_to_string(&path).map_err(|e| {
            AppError::Io(std::io::Error::new(e.kind(), format!("读取 checkpoint 失败: {}", e)))
        })?;
        let context: PipelineContext = serde_json::from_str(&data).map_err(|e| {
            AppError::Config(format!("反序列化 checkpoint 失败: {}", e))
        })?;
        Ok(Some(context))
    }

    /// 清除所有 pipeline 数据
    pub fn clear(&self) -> Result<(), AppError> {
        if self.base_dir.exists() {
            fs::remove_dir_all(&self.base_dir).map_err(|e| {
                AppError::Io(std::io::Error::new(e.kind(), format!("清除 pipeline 数据失败: {}", e)))
            })?;
        }
        Ok(())
    }

    /// 检测是否存在未完成的 pipeline
    pub fn has_pending_pipeline(&self) -> bool {
        let checkpoint = self.base_dir.join("checkpoint.json");
        checkpoint.exists()
    }

    /// 获取当前 pipeline 的摘要信息
    pub fn pipeline_summary(&self) -> Result<Option<String>, AppError> {
        let ctx = self.load_pipeline_context()?;
        match ctx {
            Some(ctx) => {
                let completed = ctx.stages.iter().filter(|s| s.status == StageStatus::Completed).count();
                let total = ctx.stages.len();
                Ok(Some(format!(
                    "Pipeline: {}/{} 阶段完成 (当前: 阶段 {})",
                    completed, total, ctx.current_stage
                )))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_store() -> (TempDir, PipelineContextStore) {
        let dir = TempDir::new().unwrap();
        let store = PipelineContextStore::new(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn test_store_creates_directory() {
        let (dir, store) = create_store();
        assert!(store.base_dir().exists());
        assert!(dir.path().join(".kb").join("pipeline").exists());
    }

    #[test]
    fn test_save_and_load_pipeline_context() {
        let (_, store) = create_store();
        let ctx = PipelineContext::new("test task", 3);
        store.save_pipeline_context(&ctx).unwrap();

        let loaded = store.load_pipeline_context().unwrap().unwrap();
        assert_eq!(loaded.task, "test task");
        assert_eq!(loaded.stages.len(), 3);
        assert_eq!(loaded.current_stage, 0);
    }

    #[test]
    fn test_save_and_load_stage_context() {
        let (_, store) = create_store();
        let ctx = StageContext {
            stage_index: 0,
            stage_name: "架构设计".to_string(),
            status: StageStatus::Completed,
            summary: "设计完成".to_string(),
            artifacts: vec!["design.md".to_string()],
            modified_files: vec![],
            error: None,
            metadata: HashMap::new(),
        };
        store.save_stage_context(&ctx).unwrap();

        let loaded = store.load_stage_context(0).unwrap().unwrap();
        assert_eq!(loaded.stage_name, "架构设计");
        assert_eq!(loaded.status, StageStatus::Completed);
        assert_eq!(loaded.summary, "设计完成");
        assert_eq!(loaded.artifacts, vec!["design.md"]);

        // 不存在的阶段应返回 None
        let none = store.load_stage_context(99).unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn test_save_and_load_checkpoint() {
        let (_, store) = create_store();
        let mut ctx = PipelineContext::new("test task", 5);
        ctx.current_stage = 2;
        ctx.stages[0].status = StageStatus::Completed;
        ctx.stages[1].status = StageStatus::Completed;

        store.save_checkpoint(&ctx).unwrap();

        let loaded = store.load_checkpoint().unwrap().unwrap();
        assert_eq!(loaded.current_stage, 2);
        assert_eq!(loaded.stages[0].status, StageStatus::Completed);
        assert_eq!(loaded.stages[1].status, StageStatus::Completed);
    }

    #[test]
    fn test_has_pending_pipeline() {
        let (_, store) = create_store();
        assert!(!store.has_pending_pipeline());

        let ctx = PipelineContext::new("test", 3);
        store.save_checkpoint(&ctx).unwrap();
        assert!(store.has_pending_pipeline());
    }

    #[test]
    fn test_build_context_prompt() {
        let mut ctx = PipelineContext::new("test", 3);
        // 没有完成阶段时返回空字符串
        assert!(ctx.build_context_prompt().is_empty());

        // 完成阶段 0
        ctx.stages[0].stage_name = "架构设计".to_string();
        ctx.stages[0].status = StageStatus::Completed;
        ctx.stages[0].summary = "完成了模块划分".to_string();
        ctx.stages[0].artifacts = vec!["pipeline/stage-0-architecture/design.md".to_string()];

        let prompt = ctx.build_context_prompt();
        assert!(prompt.contains("架构设计"));
        assert!(prompt.contains("完成了模块划分"));
        assert!(prompt.contains("产物文件"));
    }

    #[test]
    fn test_clear() {
        let (_, store) = create_store();
        let ctx = PipelineContext::new("test", 1);
        store.save_pipeline_context(&ctx).unwrap();
        assert!(store.base_dir().join("context.json").exists());

        store.clear().unwrap();
        assert!(!store.base_dir().exists());
    }

    #[test]
    fn test_pipeline_summary() {
        let (_, store) = create_store();
        // 没有 pipeline 时应返回 None
        assert!(store.pipeline_summary().unwrap().is_none());

        let mut ctx = PipelineContext::new("test", 5);
        ctx.stages[0].status = StageStatus::Completed;
        ctx.stages[1].status = StageStatus::Completed;
        ctx.current_stage = 2;
        store.save_pipeline_context(&ctx).unwrap();

        let summary = store.pipeline_summary().unwrap().unwrap();
        assert!(summary.contains("2/5"));
    }

    #[test]
    fn test_stage_dir_path() {
        let (_, store) = create_store();
        let dir = store.stage_dir(0);
        assert!(dir.to_string_lossy().ends_with("stage-0"));

        let dir5 = store.stage_dir(5);
        assert!(dir5.to_string_lossy().ends_with("stage-5"));
    }

    #[test]
    fn test_serialize_deserialize_full_pipeline() {
        let mut ctx = PipelineContext::new("full test", 3);
        ctx.stages[0].stage_name = "设计".to_string();
        ctx.stages[0].status = StageStatus::Completed;
        ctx.stages[0].summary = "设计完成".to_string();
        ctx.stages[0].modified_files = vec!["src/main.rs".to_string()];
        ctx.stages[0].metadata.insert("key".to_string(), serde_json::json!("value"));
        ctx.current_stage = 1;

        let json = serde_json::to_string_pretty(&ctx).unwrap();
        let deserialized: PipelineContext = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.task, "full test");
        assert_eq!(deserialized.stages[0].stage_name, "设计");
        assert_eq!(deserialized.stages[0].modified_files, vec!["src/main.rs"]);
        assert_eq!(
            deserialized.stages[0].metadata.get("key").unwrap().as_str().unwrap(),
            "value"
        );
    }
}
use std::sync::{Arc, Mutex};

use once_cell::sync::Lazy;
use super::{ToolArgs, ToolContext, ToolDefinition, ToolResult};
use crate::orchestrator::DependencyGraph;
use crate::utils::error::AppError;

#[derive(Clone)]
pub struct TaskManager {
    graph: Arc<Mutex<DependencyGraph>>,
    is_paused: Arc<Mutex<bool>>,
    is_cancelled: Arc<Mutex<bool>>,
}

impl TaskManager {
    #[allow(dead_code)]
    pub fn new(graph: DependencyGraph) -> Self {
        Self {
            graph: Arc::new(Mutex::new(graph)),
            is_paused: Arc::new(Mutex::new(false)),
            is_cancelled: Arc::new(Mutex::new(false)),
        }
    }

    pub fn graph(&self) -> Arc<Mutex<DependencyGraph>> {
        self.graph.clone()
    }

    #[allow(dead_code)]
    pub fn is_paused(&self) -> Arc<Mutex<bool>> {
        self.is_paused.clone()
    }

    #[allow(dead_code)]
    pub fn is_cancelled(&self) -> Arc<Mutex<bool>> {
        self.is_cancelled.clone()
    }

    pub fn pause(&self) {
        *self.is_paused.lock().unwrap() = true;
    }

    pub fn resume(&self) {
        *self.is_paused.lock().unwrap() = false;
    }

    pub fn cancel(&self) {
        *self.is_cancelled.lock().unwrap() = true;
    }

    #[allow(dead_code)]
    pub fn reset(&self) {
        *self.is_paused.lock().unwrap() = false;
        *self.is_cancelled.lock().unwrap() = false;
    }
}

pub fn task_status_tool() -> ToolDefinition {
    ToolDefinition {
        name: "task_status".to_string(),
        description: "查询当前任务状态和进度".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        skip_security: true,
        handler: Box::new(task_status_handler),
    }
}

fn task_status_handler(_args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    let content = if let Some(manager) = get_global_task_manager() {
        let graph_arc = manager.graph();
        let graph = graph_arc.lock().unwrap();
        let summary = graph.progress_summary();
        let total = graph.total_count();
        let completed = graph.completed_count();
        drop(graph);
        format!(
            "任务状态:\n\
             - 总任务数: {}\n\
             - 已完成: {}\n\
             - 暂停: {}\n\
             - 已取消: {}\n\
             \n{}",
            total,
            completed,
            *manager.is_paused.lock().unwrap(),
            *manager.is_cancelled.lock().unwrap(),
            summary,
        )
    } else {
        "当前没有正在运行的任务".to_string()
    };

    Ok(ToolResult {
        success: true,
        content,
        security_evaluation: None,
        restart_requested: false,
    })
}

pub fn pause_task_tool() -> ToolDefinition {
    ToolDefinition {
        name: "pause_task".to_string(),
        description: "暂停当前运行的任务，保存检查点".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        skip_security: true,
        handler: Box::new(pause_task_handler),
    }
}

fn pause_task_handler(_args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    if let Some(manager) = get_global_task_manager() {
        manager.pause();
        Ok(ToolResult {
            success: true,
            content: "任务已暂停，检查点已保存".to_string(),
            security_evaluation: None,
            restart_requested: false,
        })
    } else {
        Ok(ToolResult {
            success: false,
            content: "当前没有正在运行的任务".to_string(),
            security_evaluation: None,
            restart_requested: false,
        })
    }
}

pub fn resume_task_tool() -> ToolDefinition {
    ToolDefinition {
        name: "resume_task".to_string(),
        description: "从检查点恢复已暂停的任务".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        skip_security: true,
        handler: Box::new(resume_task_handler),
    }
}

fn resume_task_handler(_args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    if let Some(manager) = get_global_task_manager() {
        manager.resume();
        Ok(ToolResult {
            success: true,
            content: "任务已恢复".to_string(),
            security_evaluation: None,
            restart_requested: false,
        })
    } else {
        Ok(ToolResult {
            success: false,
            content: "当前没有已暂停的任务".to_string(),
            security_evaluation: None,
            restart_requested: false,
        })
    }
}

pub fn cancel_task_tool() -> ToolDefinition {
    ToolDefinition {
        name: "cancel_task".to_string(),
        description: "取消当前任务".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {}
        }),
        skip_security: true,
        handler: Box::new(cancel_task_handler),
    }
}

fn cancel_task_handler(_args: &ToolArgs, _context: &ToolContext) -> Result<ToolResult, AppError> {
    if let Some(manager) = get_global_task_manager() {
        manager.cancel();
        Ok(ToolResult {
            success: true,
            content: "任务已取消".to_string(),
            security_evaluation: None,
            restart_requested: false,
        })
    } else {
        Ok(ToolResult {
            success: false,
            content: "当前没有正在运行的任务".to_string(),
            security_evaluation: None,
            restart_requested: false,
        })
    }
}

static GLOBAL_TASK_MANAGER: Lazy<Mutex<Option<TaskManager>>> = Lazy::new(|| Mutex::new(None));

#[allow(dead_code)]
pub fn set_global_task_manager(manager: TaskManager) {
    *GLOBAL_TASK_MANAGER.lock().unwrap() = Some(manager);
}

pub fn get_global_task_manager() -> Option<TaskManager> {
    GLOBAL_TASK_MANAGER.lock().unwrap().clone()
}

#[allow(dead_code)]
pub fn clear_global_task_manager() {
    *GLOBAL_TASK_MANAGER.lock().unwrap() = None;
}
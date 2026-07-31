//! 会话管理处理器（Phase 3 完善）。
//!
//! 提供会话列表、详情、删除和重命名的 API，复用 `persist::SessionStore`
//! 读取 `.dev-assistant-store/` 下的 JSONL 会话数据。
//! 会话标题存于 `.dev-assistant-store/titles.json`（文件名与 ID 关联，不可改）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::persist::{SessionEvent, SessionStore};
use crate::web::AppState;

/// 会话摘要
#[derive(Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub message_count: usize,
}

/// 会话详情
#[derive(Serialize)]
pub struct SessionDetail {
    pub id: String,
    pub events: Vec<serde_json::Value>,
}

/// 重命名请求体
#[derive(Deserialize)]
pub struct RenameRequest {
    pub title: String,
}

/// 标题元数据文件路径。
fn titles_path(working_dir: &Path) -> PathBuf {
    working_dir.join(".dev-assistant-store").join("titles.json")
}

/// 读取全部会话标题映射。
fn load_titles(working_dir: &Path) -> HashMap<String, String> {
    let path = titles_path(working_dir);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
        .unwrap_or_default()
}

/// 写入全部会话标题映射。
fn save_titles(working_dir: &Path, titles: &HashMap<String, String>) -> bool {
    let path = titles_path(working_dir);
    match serde_json::to_string_pretty(titles) {
        Ok(json) => std::fs::write(&path, json).is_ok(),
        Err(_) => false,
    }
}

/// 从 JSONL 文件名提取会话 ID（`session_{timestamp}.jsonl` → `{timestamp}`）。
fn session_id_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.strip_prefix("session_").unwrap_or(s).to_string())
        .unwrap_or_default()
}

/// 事件的时间戳（所有变体都带 `timestamp` 字段）。
fn event_timestamp(event: &SessionEvent) -> &str {
    match event {
        SessionEvent::UserMessage { timestamp, .. }
        | SessionEvent::AssistantMessage { timestamp, .. }
        | SessionEvent::SystemMessage { timestamp, .. }
        | SessionEvent::ToolCallRequest { timestamp, .. }
        | SessionEvent::ToolResult { timestamp, .. }
        | SessionEvent::Compression { timestamp, .. } => timestamp,
    }
}

/// 会话文件的完整路径（带路径遍历防护）。
fn session_path(working_dir: &Path, id: &str) -> PathBuf {
    // 只允许取文件名部分，杜绝 `../` 路径遍历
    let safe_id = Path::new(id)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    working_dir
        .join(".dev-assistant-store")
        .join(format!("session_{}.jsonl", safe_id))
}

/// 获取会话列表。
///
/// `GET /api/sessions`
pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<SessionSummary>> {
    let Ok(paths) = SessionStore::list_sessions(&state.working_dir) else {
        return Json(Vec::new());
    };

    let mut sessions = Vec::new();
    let titles = load_titles(&state.working_dir);
    for path in paths {
        let id = session_id_from_path(&path);
        // 读取事件以统计创建时间与消息数
        let (created_at, message_count) = match SessionStore::read_events(&path) {
            Ok(events) => {
                let created = events
                    .first()
                    .map(event_timestamp)
                    .unwrap_or("")
                    .to_string();
                let count = events
                    .iter()
                    .filter(|e| {
                        matches!(
                            e,
                            SessionEvent::UserMessage { .. } | SessionEvent::AssistantMessage { .. }
                        )
                    })
                    .count();
                (created, count)
            }
            Err(_) => (String::new(), 0),
        };

        let title = titles.get(&id).cloned().unwrap_or_default();
        sessions.push(SessionSummary {
            id,
            title,
            created_at,
            message_count,
        });
    }

    // 按文件名排序（即按时间升序），最新的放在最前
    sessions.reverse();
    Json(sessions)
}

/// 获取单条会话详情。
///
/// `GET /api/sessions/{id}`
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Json<SessionDetail> {
    let path = session_path(&state.working_dir, &id);
    let events = if path.exists() {
        SessionStore::read_events(&path).unwrap_or_default()
    } else {
        Vec::new()
    };

    let events: Vec<serde_json::Value> = events
        .iter()
        .filter_map(|e| serde_json::to_value(e).ok())
        .collect();

    Json(SessionDetail { id, events })
}

/// 删除会话。
///
/// `DELETE /api/sessions/{id}`
pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let path = session_path(&state.working_dir, &id);
    match std::fs::remove_file(&path) {
        Ok(_) => {
            // 同步清理标题元数据
            let mut titles = load_titles(&state.working_dir);
            titles.remove(&id);
            let _ = save_titles(&state.working_dir, &titles);
            Json(serde_json::json!({"deleted": true, "id": id}))
        }
        Err(e) => Json(serde_json::json!({
            "deleted": false,
            "id": id,
            "error": e.to_string()
        })),
    }
}

/// 重命名会话。
///
/// `POST /api/sessions/{id}/rename`，请求体 `{"title": "..."}`
pub async fn rename_session(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<RenameRequest>,
) -> Json<serde_json::Value> {
    // 校验会话存在
    let path = session_path(&state.working_dir, &id);
    if !path.exists() {
        return Json(serde_json::json!({
            "success": false,
            "id": id,
            "error": "会话不存在"
        }));
    }

    // 标题去空白并限制长度，避免元数据文件被撑大
    let title = body.title.trim().chars().take(100).collect::<String>();
    if title.is_empty() {
        return Json(serde_json::json!({
            "success": false,
            "id": id,
            "error": "标题不能为空"
        }));
    }

    let mut titles = load_titles(&state.working_dir);
    titles.insert(id.clone(), title.clone());
    if save_titles(&state.working_dir, &titles) {
        Json(serde_json::json!({
            "success": true,
            "id": id,
            "title": title
        }))
    } else {
        Json(serde_json::json!({
            "success": false,
            "id": id,
            "error": "标题保存失败"
        }))
    }
}

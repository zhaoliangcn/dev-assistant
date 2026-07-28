//! 会话管理处理器（Phase 3 完善）。
//!
//! 当前提供会话列表、详情和删除的 API 桩。
//! 复用 `persist::SessionStore` 读取 JSONL 会话数据。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;

use crate::web::AppState;

/// 会话摘要
#[derive(Serialize)]
pub struct SessionSummary {
    pub id: String,
    pub created_at: String,
    pub message_count: usize,
}

/// 会话详情
#[derive(Serialize)]
pub struct SessionDetail {
    pub id: String,
    pub events: Vec<serde_json::Value>,
}

/// 获取会话列表。
///
/// `GET /api/sessions`
pub async fn list_sessions(
    State(_state): State<Arc<AppState>>,
) -> Json<Vec<SessionSummary>> {
    Json(Vec::new())
}

/// 获取单条会话详情。
///
/// `GET /api/sessions/{id}`
pub async fn get_session(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<SessionDetail> {
    Json(SessionDetail {
        id,
        events: Vec::new(),
    })
}

/// 删除会话。
///
/// `DELETE /api/sessions/{id}`
pub async fn delete_session(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"deleted": true, "id": id}))
}
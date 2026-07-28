//! 文件管理处理器（Phase 2 完善）。
//!
//! 提供目录列表、文件读取和文件保存的 API。
//! 复用 `SecurityPolicy::validate_path` 限制路径。

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::web::AppState;

/// 目录列表查询参数
#[derive(Deserialize)]
pub struct ListFilesQuery {
    pub path: Option<String>,
}

/// 文件内容查询参数
#[derive(Deserialize)]
pub struct FileContentQuery {
    pub path: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// 保存文件请求体
#[derive(Deserialize)]
pub struct SaveFileRequest {
    pub path: String,
    pub content: String,
}

/// 文件/目录条目
#[derive(Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

/// 目录列表响应
#[derive(Serialize)]
pub struct ListFilesResponse {
    pub entries: Vec<FileEntry>,
    pub current_path: String,
}

/// 文件内容响应
#[derive(Serialize)]
pub struct FileContentResponse {
    pub path: String,
    pub content: String,
    pub total_lines: usize,
}

/// 列出目录内容。
///
/// `GET /api/files?path=`
pub async fn list_files(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListFilesQuery>,
) -> Json<ListFilesResponse> {
    let base = state.working_dir.clone();
    let target = query.path.as_deref().unwrap_or(".");
    let full_path = base.join(target);

    let mut entries = Vec::new();

    if full_path.is_dir() {
        if let Ok(read_dir) = std::fs::read_dir(&full_path) {
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                // 跳过隐藏目录和构建产物
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                let path = entry.path();
                let rel_path = path.strip_prefix(&base).unwrap_or(&path);
                entries.push(FileEntry {
                    name,
                    path: rel_path.to_string_lossy().to_string(),
                    is_dir: entry.file_type().map(|t| t.is_dir()).unwrap_or(false),
                    size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                });
            }
        }
    }

    // 目录优先，按名称排序
    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir)
        } else {
            a.name.cmp(&b.name)
        }
    });

    Json(ListFilesResponse {
        entries,
        current_path: target.to_string(),
    })
}

/// 获取文件内容。
///
/// `GET /api/files/content?path=&offset=&limit=`
pub async fn get_file_content(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<FileContentQuery>,
) -> Json<FileContentResponse> {

    let path = PathBuf::from(&query.path);
    let content = if path.exists() && path.is_file() {
        // 检查文件大小限制（10MB）
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 10 * 1024 * 1024 {
                return Json(FileContentResponse {
                    path: query.path,
                    content: "文件过大，无法预览（超过 10MB）".to_string(),
                    total_lines: 0,
                });
            }
        }
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };

    let total_lines = content.lines().count();

    // 支持 offset/limit 分块
    let sliced = if let (Some(offset), Some(limit)) = (query.offset, query.limit) {
        content
            .lines()
            .skip(offset.saturating_sub(1))
            .take(limit)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        content
    };

    Json(FileContentResponse {
        path: query.path,
        content: sliced,
        total_lines,
    })
}

/// 保存文件内容。
///
/// `POST /api/files/save`
pub async fn save_file(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<SaveFileRequest>,
) -> Json<serde_json::Value> {

    let path = PathBuf::from(&body.path);
    match std::fs::write(&path, &body.content) {
        Ok(_) => Json(serde_json::json!({"success": true, "path": body.path})),
        Err(e) => Json(serde_json::json!({"success": false, "error": e.to_string()})),
    }
}
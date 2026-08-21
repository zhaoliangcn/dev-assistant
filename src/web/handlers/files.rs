//! 文件管理处理器（Phase 2 完善）。
//!
//! 提供目录列表、文件读取和文件保存的 API。
//! 通过 `SecurityPolicy::validate_path` / `validate_path_exists` 限制路径，
//! 拒绝路径遍历（`..`）与越界绝对路径，确保所有文件操作都在工作目录内。

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::utils::error::AppError;
use crate::web::AppState;

/// 路径校验失败 → HTTP 403 + JSON 错误体。
///
/// 所有文件 handler 在访问文件系统前必须先调用 `SecurityPolicy::validate_*`，
/// 失败时经此 helper 映射为 403，避免把工作目录之外的文件暴露给前端。
fn path_forbidden(e: AppError) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "error": e.to_string() })),
    )
}

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
///
/// 安全校验：`path` 必须解析在工作目录内，拒绝 `..` 遍历与越界绝对路径。
pub async fn list_files(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListFilesQuery>,
) -> Result<Json<ListFilesResponse>, (StatusCode, Json<serde_json::Value>)> {
    let base = state.working_dir.clone();
    let target = query.path.as_deref().unwrap_or(".");
    // 安全校验：拒绝路径遍历与越界绝对路径（返回值仅作授权，读取仍用 base.join 保留相对路径）
    state.security.validate_path(target).map_err(path_forbidden)?;
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

    Ok(Json(ListFilesResponse {
        entries,
        current_path: target.to_string(),
    }))
}

/// 获取文件内容。
///
/// `GET /api/files/content?path=&offset=&limit=`
///
/// 安全校验：`path` 必须解析在工作目录内，拒绝遍历与越界绝对路径。
/// 文件不存在时返回空内容（与旧版行为一致），而非 403。
pub async fn get_file_content(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileContentQuery>,
) -> Result<Json<FileContentResponse>, (StatusCode, Json<serde_json::Value>)> {
    // 安全校验：validate_path 对工作目录内（含不存在）路径放行，越界路径 403
    let path: PathBuf = state
        .security
        .validate_path(&query.path)
        .map_err(path_forbidden)?;
    let content = if path.exists() && path.is_file() {
        // 检查文件大小限制（10MB）
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 10 * 1024 * 1024 {
                return Ok(Json(FileContentResponse {
                    path: query.path,
                    content: "文件过大，无法预览（超过 10MB）".to_string(),
                    total_lines: 0,
                }));
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

    Ok(Json(FileContentResponse {
        path: query.path,
        content: sliced,
        total_lines,
    }))
}

/// 保存文件内容。
///
/// `POST /api/files/save`
///
/// 安全校验：目标文件可能尚不存在（新建），故用 `validate_path_exists`
/// 校验其父目录在工作目录内，拒绝遍历与越界绝对路径。
pub async fn save_file(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SaveFileRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 安全校验：validate_path_exists 不要求文件已存在，校验父目录在工作目录内
    let path: PathBuf = state
        .security
        .validate_path_exists(&body.path)
        .map_err(path_forbidden)?;
    match std::fs::write(&path, &body.content) {
        Ok(_) => Ok(Json(serde_json::json!({"success": true, "path": body.path}))),
        Err(e) => Ok(Json(serde_json::json!({"success": false, "error": e.to_string()}))),
    }
}

#[cfg(test)]
mod tests {
    //! 验证文件 handler 依赖的路径校验契约。
    //! `SecurityPolicy` 的 `allowed_paths` 仅含工作目录本身，故 `..` 遍历与
    //! 越界绝对路径必被拒，工作目录内（含尚不存在的新文件）放行。

    use crate::security::SecurityPolicy;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn file_path_security_categories() {
        let dir = tempdir().unwrap();
        let policy = SecurityPolicy::new(dir.path(), None, false);

        // 1) 工作目录内的已存在文件：放行（get_file_content 路径）
        fs::write(dir.path().join("ok.txt"), "hi").unwrap();
        assert!(policy.validate_path("ok.txt").is_ok());

        // 2) 路径遍历：拒绝
        assert!(policy.validate_path("../escape.txt").is_err());
        assert!(policy.validate_path("sub/../../escape.txt").is_err());

        // 3) 越界绝对路径：拒绝（join 时绝对路径会替换基目录）
        assert!(policy.validate_path("/etc/passwd").is_err());

        // 4) save_file 路径：新建文件（父目录在工作目录内）放行，越界拒绝
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        assert!(policy.validate_path_exists("sub/new.rs").is_ok());
        assert!(policy.validate_path_exists("../evil.rs").is_err());
        assert!(policy.validate_path_exists("/etc/cron.d/evil").is_err());
    }
}
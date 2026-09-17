//! 技能管理处理器。
//!
//! 提供 Web 界面的技能安装/列表/移除/预览 API，
//! 底层复用 `crate::skills::installer` 的安装逻辑（与 REPL `/skill` 命令一致）。
//!
//! 安全边界：
//! - `source` 协议白名单（https/http/git/ssh），拒绝 `file://` 等协议
//! - 移除时的技能名校验，拒绝路径遍历（`..`、`/`、`\`）

use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::skills::installer::{
    install_skill, list_skills, read_skill_meta, remove_skill, InstallScope,
};
use crate::web::AppState;

/// 错误映射：AppError → HTTP 400 + JSON 错误体。
fn bad_request(e: crate::utils::error::AppError) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": e.to_string() })),
    )
}

/// 校验技能名：仅允许字母数字、连字符、下划线、点，防止路径遍历。
fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("技能名不能为空".into());
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err(format!("非法技能名: {}", name));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!("非法技能名（仅允许字母数字-_.）: {}", name));
    }
    Ok(())
}

/// 校验 Git source 协议白名单；本地目录（相对路径或绝对路径）交由 installer 处理。
fn validate_source(source: &str) -> Result<(), String> {
    let lower = source.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("git://")
        || lower.starts_with("ssh://")
        || lower.starts_with("git@")
    {
        return Ok(());
    }
    // owner/repo 简写：不含协议头且不含空格
    if !source.contains("://") && !source.contains(' ') && source.chars().all(|c| c.is_ascii_graphic())
    {
        return Ok(());
    }
    Err(format!("不支持的 source 格式: {}", source))
}

// ── 请求/响应类型 ──

#[derive(Deserialize)]
pub struct ListSkillsQuery {
    /// project | global | all（默认 all）
    pub scope: Option<String>,
}

#[derive(Deserialize)]
pub struct InstallRequest {
    /// Git 仓库（owner/repo 或完整 URL）或本地目录
    pub source: String,
    /// 可选：只安装指定名称的技能
    #[serde(default)]
    pub skill_names: Option<Vec<String>>,
    /// project（默认）| global
    pub scope: Option<String>,
}

#[derive(Deserialize)]
pub struct RemoveQuery {
    /// project | global（默认 project）
    pub scope: Option<String>,
}

#[derive(Serialize)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub version: Option<String>,
    pub scope: String,
    /// 安装来源（git_url 或本地路径，来自 .skill-meta.json）
    pub source: Option<String>,
}

#[derive(Serialize)]
pub struct ListSkillsResponse {
    pub skills: Vec<SkillEntry>,
}

#[derive(Serialize)]
pub struct InstallResponse {
    pub success: bool,
    pub installed: Vec<SkillEntry>,
}

fn scope_from_str(s: &str) -> InstallScope {
    if s.eq_ignore_ascii_case("global") {
        InstallScope::Global
    } else {
        InstallScope::Project
    }
}

fn skill_to_entry(skill: &crate::skills::Skill, scope: &str, dir: Option<&std::path::Path>) -> SkillEntry {
    let meta = dir.and_then(read_skill_meta);
    SkillEntry {
        name: skill.meta.name.clone(),
        description: skill.meta.description.clone(),
        when_to_use: skill.meta.when_to_use.clone(),
        version: skill.meta.version.clone(),
        scope: scope.to_string(),
        source: meta.map(|m| m.git_url.unwrap_or_else(|| m.source_path.unwrap_or_default())),
    }
}

/// `GET /api/skills?scope=`
pub async fn get_skills(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListSkillsQuery>,
) -> Result<Json<ListSkillsResponse>, (StatusCode, Json<serde_json::Value>)> {
    let scope_str = query.scope.as_deref().unwrap_or("all");
    let scopes: Vec<(&str, InstallScope)> = match scope_str {
        "project" => vec![("project", InstallScope::Project)],
        "global" => vec![("global", InstallScope::Global)],
        _ => vec![
            ("project", InstallScope::Project),
            ("global", InstallScope::Global),
        ],
    };

    let mut skills = Vec::new();
    for (name, scope) in scopes {
        let dir = match scope {
            InstallScope::Global => crate::skills::installer::global_skills_dir(),
            InstallScope::Project => crate::skills::installer::project_skills_dir(&state.working_dir),
        };
        if let Ok(list) = list_skills(scope, &state.working_dir) {
            for skill in list {
                skills.push(skill_to_entry(&skill, name, Some(&dir.join(&skill.meta.name))));
            }
        }
    }

    Ok(Json(ListSkillsResponse { skills }))
}

/// `POST /api/skills/install`
pub async fn install(
    State(state): State<Arc<AppState>>,
    Json(body): Json<InstallRequest>,
) -> Result<Json<InstallResponse>, (StatusCode, Json<serde_json::Value>)> {
    validate_source(&body.source).map_err(|e| bad_request(crate::utils::error::AppError::Config(e)))?;
    if let Some(filters) = &body.skill_names {
        for name in filters {
            validate_skill_name(name)
                .map_err(|e| bad_request(crate::utils::error::AppError::Config(e)))?;
        }
    }
    let scope = scope_from_str(body.scope.as_deref().unwrap_or("project"));

    let working_dir = state.working_dir.clone();
    let source = body.source.clone();
    let filters = body.skill_names.clone();

    // install_skill 内部含阻塞的 git clone，放到阻塞线程池执行
    let result = tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async {
            install_skill(&source, scope, &working_dir, filters.as_deref()).await
        })
    })
    .await
    .map_err(|e| bad_request(crate::utils::error::AppError::Config(format!("安装任务失败: {}", e))))?;

    match result {
        Ok(installed) => {
            let scope_label = if scope == InstallScope::Global { "global" } else { "project" };
            let entries: Vec<SkillEntry> = installed
                .iter()
                .map(|s| skill_to_entry(s, scope_label, None))
                .collect();
            Ok(Json(InstallResponse { success: true, installed: entries }))
        }
        Err(e) => Err(bad_request(e)),
    }
}

/// `DELETE /api/skills/{name}?scope=`
pub async fn remove(
    State(state): State<Arc<AppState>>,
    AxumPath(name): AxumPath<String>,
    Query(query): Query<RemoveQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    validate_skill_name(&name).map_err(|e| bad_request(crate::utils::error::AppError::Config(e)))?;
    let scope = scope_from_str(query.scope.as_deref().unwrap_or("project"));

    match remove_skill(&name, scope, &state.working_dir) {
        Ok(_) => Ok(Json(serde_json::json!({ "success": true }))),
        Err(e) => Err(bad_request(e)),
    }
}

#[derive(Deserialize)]
pub struct PreviewRequest {
    pub source: String,
}

#[derive(Serialize)]
pub struct PreviewResponse {
    pub skills: Vec<SkillEntry>,
}

/// `POST /api/skills/preview` — 克隆 source 到临时目录，列出可安装的技能（不安装）。
pub async fn preview(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<PreviewRequest>,
) -> Result<Json<PreviewResponse>, (StatusCode, Json<serde_json::Value>)> {
    validate_source(&body.source).map_err(|e| bad_request(crate::utils::error::AppError::Config(e)))?;

    let source = body.source.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::skills::installer::preview_skills(&source)
    })
    .await
    .map_err(|e| bad_request(crate::utils::error::AppError::Config(format!("预览任务失败: {}", e))))?;

    match result {
        Ok(skills) => {
            let entries: Vec<SkillEntry> = skills
                .iter()
                .map(|s| skill_to_entry(s, "preview", None))
                .collect();
            Ok(Json(PreviewResponse { skills: entries }))
        }
        Err(e) => Err(bad_request(e)),
    }
}

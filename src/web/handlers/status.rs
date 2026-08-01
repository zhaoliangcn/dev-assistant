//! 状态/信息 API 处理器。
//!
//! 提供系统状态、模型列表等信息的 REST API 端点。
//! 同时提供主页面渲染。

use std::sync::Arc;

use axum::{
    extract::State,
    response::{Html, Json},
};
use serde::Serialize;

use crate::web::AppState;

/// 系统状态响应。
#[derive(Serialize)]
pub struct SystemStatus {
    pub version: String,
    pub project_dir: String,
    pub mode: String,
    pub active_model: String,
    pub online: bool,
    pub uptime: String,
}

/// 服务启动时刻（首次请求时初始化，用于 uptime 统计）。
static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// 获取系统状态。
pub async fn get_status(
    State(state): State<Arc<AppState>>,
) -> Json<SystemStatus> {
    let started = *STARTED.get_or_init(std::time::Instant::now);
    let elapsed = started.elapsed();
    let uptime = if elapsed.as_secs() >= 3600 {
        format!("{}h{:02}m", elapsed.as_secs() / 3600, (elapsed.as_secs() % 3600) / 60)
    } else if elapsed.as_secs() >= 60 {
        format!("{}m{:02}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
    } else {
        format!("{}s", elapsed.as_secs())
    };

    Json(SystemStatus {
        version: env!("CARGO_PKG_VERSION").to_string(),
        project_dir: state.working_dir.display().to_string(),
        mode: "web".to_string(),
        active_model: state.llm.active_model().to_string(),
        online: true,
        uptime,
    })
}

/// 模型信息。
#[derive(Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub provider: String,
    pub active: bool,
}

/// 切换模型请求体。
#[derive(serde::Deserialize)]
pub struct SwitchModelRequest {
    pub name: String,
}

/// 获取可用模型列表（真实数据，来自 LlmClient）。
pub async fn get_models(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<ModelInfo>> {
    let models: Vec<ModelInfo> = state
        .llm
        .list_model_info()
        .into_iter()
        .map(|(name, provider, active)| ModelInfo {
            name,
            provider,
            active,
        })
        .collect();
    Json(models)
}

/// 切换模型（真实切换，来自 LlmClient）。
pub async fn switch_model(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SwitchModelRequest>,
) -> Json<serde_json::Value> {
    match state.llm.switch_model(&body.name) {
        Ok(_) => Json(serde_json::json!({
            "success": true,
            "name": body.name,
        })),
        Err(e) => Json(serde_json::json!({
            "success": false,
            "name": body.name,
            "error": e.to_string(),
        })),
    }
}

/// 渲染主页面。
pub async fn index_page(
    State(state): State<Arc<AppState>>,
) -> Html<String> {
    render_template(&state, "index.html")
}

/// 渲染文件浏览器页面。
pub async fn files_page(
    State(state): State<Arc<AppState>>,
) -> Html<String> {
    render_template(&state, "files.html")
}

/// 公共模板渲染辅助：按模板名渲染并返回 HTML。
fn render_template(state: &AppState, name: &str) -> Html<String> {
    let html = match state.templates.get_template(name) {
        Ok(tmpl) => tmpl.render(&minijinja::Value::UNDEFINED).unwrap_or_else(|e| {
            format!("<!DOCTYPE html><html><body><h1>模板渲染失败</h1><p>{}</p></body></html>", e)
        }),
        Err(e) => {
            format!("<!DOCTYPE html><html><body><h1>模板加载失败</h1><p>{}</p></body></html>", e)
        }
    };
    Html(html)
}
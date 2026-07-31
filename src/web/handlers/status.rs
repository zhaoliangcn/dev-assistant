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

/// 获取系统状态。
pub async fn get_status(
    State(state): State<Arc<AppState>>,
) -> Json<SystemStatus> {
    Json(SystemStatus {
        version: env!("CARGO_PKG_VERSION").to_string(),
        project_dir: state.working_dir.display().to_string(),
        mode: "web".to_string(),
        active_model: "default".to_string(),
        online: true,
        uptime: format!("{}s", 0),
    })
}

/// 模型信息。
#[derive(Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub provider: String,
    pub active: bool,
}

/// 获取可用模型列表。
pub async fn get_models(
    State(_state): State<Arc<AppState>>,
) -> Json<Vec<ModelInfo>> {
    Json(vec![
        ModelInfo {
            name: "default".to_string(),
            provider: "openai".to_string(),
            active: true,
        },
    ])
}

/// 切换模型（暂为桩实现）。
pub async fn switch_model(
    State(_state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "success": true,
        "message": "模型切换功能将在后续版本实现"
    }))
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
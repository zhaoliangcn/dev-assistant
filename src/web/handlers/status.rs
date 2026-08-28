//! 状态/信息 API 处理器。
//!
//! 提供系统状态、模型列表与配置管理等 REST API 端点。
//! 同时提供主页面渲染。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{Html, Json},
};
use serde::{Deserialize, Serialize};

use crate::llm::ProviderConfig;
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

/// 模型信息（供前端展示与编辑；api_key 做脱敏，不回传完整密钥）。
#[derive(Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub provider: String,
    pub api_url: String,
    /// 脱敏后的密钥：已设置时形如 `****abcd`，未设置时为 null
    pub api_key: Option<String>,
    /// 是否已配置 API 密钥
    pub has_api_key: bool,
    pub model: String,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<usize>,
    pub active: bool,
}

/// 切换模型请求体。
#[derive(Deserialize)]
pub struct SwitchModelRequest {
    pub name: String,
}

/// 保存（新增/更新）模型配置请求体。
///
/// - `api_key` 为空且 `clear_api_key = false`：保留原有密钥（编辑时不回显）
/// - `clear_api_key = true`：清空密钥
#[derive(Deserialize)]
pub struct SaveModelRequest {
    pub name: String,
    pub provider: String,
    pub api_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_output_tokens: Option<usize>,
}

/// 密钥脱敏：`sk-abc12345` → `****12345`（保留末尾 4 位），过短则仅显示 `****`。
fn mask_api_key(key: &str) -> String {
    if key.len() > 4 {
        format!("****{}", &key[key.len() - 4..])
    } else {
        "****".to_string()
    }
}

/// 获取可用模型列表（含完整配置信息与配置文件路径）。
pub async fn get_models(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let active_name = state.llm.active_model();
    let configs = state.llm.get_configs();
    let models: Vec<ModelInfo> = configs
        .into_iter()
        .map(|c| {
            let api_key = c.api_key.as_ref().map(|k| mask_api_key(k));
            ModelInfo {
                active: c.name == active_name,
                name: c.name,
                provider: c.provider,
                api_url: c.api_url,
                has_api_key: api_key.is_some(),
                api_key,
                model: c.model,
                temperature: c.temperature,
                max_output_tokens: c.max_output_tokens,
            }
        })
        .collect();
    Json(serde_json::json!({
        "models": models,
        "config_path": state.models_config_path.display().to_string(),
    }))
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

/// 保存（新增或更新）模型配置：先更新运行时 LlmClient，再持久化到配置文件。
pub async fn save_model(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SaveModelRequest>,
) -> Json<serde_json::Value> {
    let cfg = ProviderConfig {
        name: body.name.clone(),
        provider: body.provider.clone(),
        api_url: body.api_url.clone(),
        api_key: body.api_key.clone(),
        model: body.model.clone(),
        temperature: body.temperature,
        max_output_tokens: body.max_output_tokens,
    };

    let result = match state.llm.add_or_update_config(&cfg, body.clear_api_key) {
        Ok(_) => state.llm.save_to_file(&state.models_config_path).await,
        Err(e) => Err(e),
    };
    match result {
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

/// 删除模型配置：先从运行时移除，再持久化到配置文件。
pub async fn delete_model(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Json<serde_json::Value> {
    let result = match state.llm.remove_config(&name) {
        Ok(_) => state.llm.save_to_file(&state.models_config_path).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(_) => Json(serde_json::json!({
            "success": true,
            "name": name,
        })),
        Err(e) => Json(serde_json::json!({
            "success": false,
            "name": name,
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
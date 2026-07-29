//! axum Router 定义。
//!
//! 集中定义所有路由和中间件链。

use std::sync::Arc;

use axum::{
    Router,
    middleware,
    routing::{get, post, delete},
};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::info;

use crate::web::handlers;
use crate::web::AppState;

/// 构建完整的 axum Router。
///
/// 包含：
/// - 静态资源服务（开发模式从磁盘加载，生产模式从 rust-embed 加载）
/// - REST API 路由
/// - WebSocket 路由
/// - 日志和错误处理中间件
pub fn build_router(state: AppState) -> Router {
    let working_dir = state.working_dir.clone();
    let state = Arc::new(state);

    // ── API 路由 ──
    let api_routes = Router::new()
        // 状态/信息
        .route("/api/status", get(handlers::status::get_status))
        .route("/api/models", get(handlers::status::get_models))
        .route("/api/models/switch", post(handlers::status::switch_model))
        // 会话管理
        .route("/api/sessions", get(handlers::session::list_sessions))
        .route("/api/sessions/{id}", get(handlers::session::get_session))
        .route("/api/sessions/{id}", delete(handlers::session::delete_session))
        // 文件管理
        .route("/api/files", get(handlers::files::list_files))
        .route("/api/files/content", get(handlers::files::get_file_content))
        .route("/api/files/save", post(handlers::files::save_file));

    // ── WebSocket 路由 ──
    let ws_routes = Router::new()
        .route("/ws/chat", get(handlers::chat::ws_handler));

    // ── 静态资源 ──
    // 开发模式：从磁盘加载静态文件
    let static_dir = working_dir.join("src/web/static");
    let static_service = if static_dir.exists() {
        info!("使用开发模式静态文件服务: {}", static_dir.display());
        ServeDir::new(&static_dir)
    } else {
        // 生产模式：使用嵌入的静态资源
        info!("使用嵌入的静态资源");
        ServeDir::new("")
    };

    // ── 主页面路由 ──
    let page_routes = Router::new()
        .route("/", get(handlers::status::index_page));

    // ── 合并所有路由 ──
    Router::new()
        .merge(page_routes)
        .merge(api_routes)
        .merge(ws_routes)
        .nest_service("/static", static_service)
        // 开发模式禁用静态资源缓存
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache, no-store, must-revalidate"),
        ))
        // 中间件
        .layer(CorsLayer::permissive())
        .with_state(state)
}
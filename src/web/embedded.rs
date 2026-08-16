//! 生产模式静态资源嵌入（rust-embed）。
//!
//! 开发模式从磁盘 `src/web/static/` 加载；生产模式通过编译时嵌入，
//! 将 HTML/CSS/JS/fonts 等与二进制打包，免去运行时磁盘依赖。

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// 编译时嵌入的静态资源目录。
///
/// 路径相对于 `Cargo.toml` 所在目录（即项目根）。
/// `mime-guess` feature 按扩展名自动推断 Content-Type。
#[derive(RustEmbed)]
#[folder = "src/web/static"]
struct Asset;

/// 从嵌入资源中提取文件并返回（含 Content-Type 与长缓存头）。
///
/// 若路径以 `/` 开头则截掉前缀；拼上 `index.html` 做目录兜底。
/// 未找到时返回 404 而非 panic。
pub async fn serve_embedded(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // 尝试直接匹配；若路径为空（`/static` 根）或目录，回退到 index.html
    let asset = Asset::get(path)
        .or_else(|| Asset::get(&format!("{}/index.html", path)))
        .or_else(|| Asset::get("index.html"));

    match asset {
        Some(content) => {
            let mime = mime_guess(&path);
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .header(
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable",
                )
                .body(axum::body::Body::from(content.data))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => {
            // 404 页：嵌入资源中不存在时返回纯文本
            (
                StatusCode::NOT_FOUND,
                [(
                    header::CONTENT_TYPE,
                    "text/plain; charset=utf-8",
                )],
                format!("资源未找到: {}", uri.path()),
            )
                .into_response()
        }
    }
}

/// 按文件扩展名推断 MIME 类型（回退到 `application/octet-stream`）。
fn mime_guess(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else {
        "application/octet-stream"
    }
}
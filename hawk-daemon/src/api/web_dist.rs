//! 局域网 web 查看的静态资源托管（Electron 传入 web/dist 时启用）。
//! SPA 回退到 index.html；/assets/ 下 vite 产物带内容哈希 immutable 长缓存，
//! index.html 与其余路径 no-cache（避免启发式缓存旧 HTML）。

use crate::api::SharedState;
use axum::extract::State;
use axum::http::Uri;
use axum::response::{IntoResponse, Response};

/// 静态资源 + SPA 回退服务（挂载为 fallback_service；/api 与 /health 由正常路由先行匹配）
pub async fn serve(State(state): State<SharedState>, uri: Uri) -> Response {
    let Some(web_dist) = &state.settings.web_dist else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    let path = uri.path().trim_start_matches('/');
    let candidate = join(web_dist, path);
    let file = if !path.is_empty() && std::path::Path::new(&candidate).is_file() {
        candidate
    } else {
        let index = join(web_dist, "index.html");
        if std::path::Path::new(&index).is_file() {
            index
        } else {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    };

    let bytes = match std::fs::read(&file) {
        Ok(b) => b,
        Err(_) => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };
    let cache_control = if uri.path().starts_with("/assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let content_type = mime_guess::from_path(&file)
        .first_or_octet_stream()
        .to_string();
    Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", content_type)
        .header("cache-control", cache_control)
        .body(axum::body::Body::from(bytes))
        .unwrap()
}

fn join(base: &str, child: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{child}")
    } else {
        format!("{base}/{child}")
    }
}

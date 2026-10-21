//! 局域网 web 查看的静态资源托管（Electron 传入 web/dist 时启用）。
//! SPA 回退到 index.html；/assets/ 下 vite 产物带内容哈希 immutable 长缓存，
//! index.html 与其余路径 no-cache（避免启发式缓存旧 HTML）。

use crate::api::SharedState;
use axum::extract::State;
use axum::http::Uri;
use axum::response::{IntoResponse, Response};
use std::path::PathBuf;

/// 静态资源 + SPA 回退服务（挂载为 fallback_service；/api 与 /health 由正常路由先行匹配）
pub async fn serve(State(state): State<SharedState>, uri: Uri) -> Response {
    let Some(web_dist) = &state.settings.web_dist else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    let request_path = uri.path().trim_start_matches('/');
    let Some(candidate) = resolve(web_dist, request_path) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    let file = if !request_path.is_empty() && candidate.is_file() {
        candidate
    } else {
        let index = PathBuf::from(web_dist).join("index.html");
        if index.is_file() {
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

/// 请求路径 → 产物内的文件路径。`.`/`..` 与反斜杠一律拒绝，防越出 web_dist
/// （网页版经服务端隧道暴露，静态托管面对公网，必须挡住 `..` 穿越）
fn resolve(web_dist: &str, request_path: &str) -> Option<PathBuf> {
    let mut file = PathBuf::from(web_dist);
    for segment in request_path.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment.contains('\\') {
            return None;
        }
        file.push(segment);
    }
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn resolve_joins_normal_paths() {
        assert_eq!(
            resolve("/dist", "assets/index-abc.js"),
            Some("/dist/assets/index-abc.js".into())
        );
        assert_eq!(resolve("/dist", ""), Some("/dist".into()));
        assert_eq!(resolve("/dist", "a//b"), Some("/dist/a/b".into()));
    }

    #[test]
    fn resolve_rejects_traversal() {
        assert_eq!(resolve("/dist", "../../etc/passwd"), None);
        assert_eq!(resolve("/dist", "assets/../../secret"), None);
        assert_eq!(resolve("/dist", "..\\..\\secret"), None);
    }
}

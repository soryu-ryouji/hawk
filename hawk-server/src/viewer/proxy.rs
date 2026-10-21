//! 隧道代理：`/api/v1/*` 非控制面路径经浏览器令牌绑定，逐请求开 QUIC 流转发给 A 侧 daemon。
//! A 侧隧道端按 TunneledViewer 标记只读处理（查看面天然只读）。

use super::ViewerManager;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hyper_util::rt::TokioIo;
use std::sync::Arc;

/// 回退路由：/api/v1/* 非控制面路径 → 隧道代理；其余 404
pub async fn api_fallback(
    State(manager): State<Arc<ViewerManager>>,
    req: axum::extract::Request,
) -> Response {
    if req.uri().path().starts_with("/api/v1/") {
        return proxy(State(manager), req).await.into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

/// 回退代理：Bearer 或 ?token= 浏览器令牌 → 绑定会话 → 隧道转发
pub async fn proxy(
    State(manager): State<Arc<ViewerManager>>,
    req: axum::extract::Request,
) -> Result<axum::response::Response, StatusCode> {
    let token = bearer_or_query(&req).ok_or(StatusCode::UNAUTHORIZED)?;
    let session = manager
        .session_for(&token)
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let (send, recv) = session.conn.open_bi().await.map_err(|e| {
        tracing::debug!("隧道流打开失败: {e}");
        StatusCode::BAD_GATEWAY
    })?;
    let io = TokioIo::new(tokio::io::join(recv, send));
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!("隧道 HTTP 客户端连接结束: {e}");
        }
    });
    let resp = sender
        .send_request(req)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let (parts, body) = resp.into_parts();
    Ok(axum::response::Response::from_parts(
        parts,
        axum::body::Body::new(body),
    ))
}

fn bearer_or_query(req: &axum::extract::Request) -> Option<String> {
    let bearer = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if let Some(value) = bearer {
        return Some(value.to_string());
    }
    req.uri().query().and_then(|q| {
        q.split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(k, _)| *k == "token")
            .map(|(_, v)| v.to_string())
    })
}

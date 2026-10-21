//! 隧道代理：非控制面路径经浏览器令牌绑定，逐请求开 QUIC 流转发给 A 侧 daemon。
//! 查看面 API（`/api/v1/*`）与查看器静态资源（`/app`、`/assets/*` 等）都走这里——
//! 前端产物由 daemon 侧托管，服务端不再单独部署一份。A 侧隧道端按 TunneledPeer
//! 的会话权限处理（网页版只读，插件写会话受目标设备开关限制）。
//!
//! 凭据来源：`Authorization: Bearer` > `?token=` > `hawk_bt` cookie。cookie 只为静态子资源
//! 存在（`<script src>`、`<img>` 无法带请求头），页面 URL 带 `?token=` 时下发；`/api/*`
//! 不认 cookie，避免 cookie 自动携带带来的 CSRF 面。

use super::ViewerManager;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use hyper_util::rt::TokioIo;
use std::sync::Arc;

/// 静态子资源凭据 cookie 名
const TOKEN_COOKIE: &str = "hawk_bt";
/// 隧道内会话标识头（与 hawk-daemon remote::data::SESSION_HEADER 一致）
const SESSION_HEADER: &str = "x-hawk-session";
/// cookie 有效期（与选择器页 localStorage 记 token 的寿命对齐）
const TOKEN_COOKIE_MAX_AGE: u32 = 60 * 60 * 24 * 30;

/// 回退路由：除 `/`（设备选择器页）与已显式注册的控制面/查看面路由外，全部经隧道转发
pub async fn fallback(
    State(manager): State<Arc<ViewerManager>>,
    req: axum::extract::Request,
) -> Response {
    let navigation = is_page_navigation(&req);
    // 页面 URL 带的 token：下发给静态子资源，并用于未登录时回选择器页的跳转
    let explicit = query_token(&req).or_else(|| bearer(&req));
    let secure_cookie = is_https(&req);
    let token = explicit
        .clone()
        .or_else(|| static_cookie_token(&req).map(String::from));
    let Some(token) = token else {
        return unauthorized(navigation, None);
    };

    match forward(manager, &token, req).await {
        Ok(mut resp) => {
            // 地址栏打开 /app?token=... → 记住它，页面里的 assets/icon 请求才能通过
            if let Some(token) = explicit.filter(|_| navigation && resp.status().is_success()) {
                set_token_cookie(resp.headers_mut(), &token, secure_cookie);
            }
            resp
        }
        // 地址栏直接打开 /app（令牌失效或尚未建会话）回选择器页重新登录，
        // 而不是把裸 401 JSON 甩给浏览器；显式携带的令牌原样带回，选择器页据此免再输密钥
        Err(StatusCode::UNAUTHORIZED) if navigation => unauthorized(true, explicit),
        Err(status) => status.into_response(),
    }
}

/// 隧道转发：按令牌取会话，开一条 QUIC 双向流当 HTTP/1.1 连接把请求送过去
async fn forward(
    manager: Arc<ViewerManager>,
    token: &str,
    req: axum::extract::Request,
) -> Result<Response, StatusCode> {
    let session = manager.session_for(token).ok_or(StatusCode::UNAUTHORIZED)?;

    // 客户端自带的会话头一律丢弃，改用服务端绑定的会话标识（A 侧据此判定读写权限）
    let (mut parts, body) = req.into_parts();
    parts.headers.remove(SESSION_HEADER);
    parts.headers.insert(
        SESSION_HEADER,
        HeaderValue::from_str(&session.hawk_session_id).map_err(|_| StatusCode::BAD_GATEWAY)?,
    );
    let req = axum::extract::Request::from_parts(parts, body);

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
    Ok(Response::from_parts(parts, axum::body::Body::new(body)))
}

/// 未登录：页面导航回选择器页（带原 token，省得再输密钥），其余保持 401
fn unauthorized(navigation: bool, token: Option<String>) -> Response {
    if !navigation {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    match token {
        Some(token) => Redirect::to(&format!("/?token={token}")).into_response(),
        None => Redirect::to("/").into_response(),
    }
}

/// 浏览器页面导航（地址栏打开）与 SPA 的 XHR/图片请求区分：只有前者在 401 时回登录页
fn is_page_navigation(req: &axum::extract::Request) -> bool {
    req.method() == axum::http::Method::GET
        && req
            .headers()
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("text/html"))
}

fn bearer(req: &axum::extract::Request) -> Option<String> {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(String::from)
}

fn query_token(req: &axum::extract::Request) -> Option<String> {
    req.uri().query().and_then(|q| {
        q.split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(k, _)| *k == "token")
            .map(|(_, v)| v.to_string())
    })
}

/// 静态子资源（非 `/api/*`）才认 cookie：API 请求一律要求显式令牌
fn static_cookie_token(req: &axum::extract::Request) -> Option<&str> {
    if req.uri().path().starts_with("/api/") {
        return None;
    }
    req.headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == TOKEN_COOKIE)
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

/// 反代是否终结了 TLS（决定 cookie 带不带 Secure；本机明文调试时不带，否则浏览器丢弃）
fn is_https(req: &axum::extract::Request) -> bool {
    req.headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        == Some("https")
}

fn set_token_cookie(headers: &mut HeaderMap, token: &str, secure: bool) {
    let cookie = format!(
        "{TOKEN_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={TOKEN_COOKIE_MAX_AGE}{}",
        if secure { "; Secure" } else { "" }
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        headers.insert(header::SET_COOKIE, value);
    }
}

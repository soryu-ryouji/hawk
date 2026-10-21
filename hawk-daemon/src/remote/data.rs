//! QUIC 数据面：A 侧隧道端 + B 侧会话代理。
//! A 侧：接受 QUIC 连接，每条双向流即一条 HTTP/1.1 连接（hyper 解析），进程内转发给
//! API 路由并标记 TunneledViewer（viewer 只读）；B 侧：环回 HTTP 代理（会话 token 门禁），
//! 每个请求开一条 QUIC 流转发。HTTP 解析两端的流式语义（SSE/缩略图）由 hyper 原生承载。

use crate::remote::identity::Identity;
use crate::remote::tls::{self, PeerAllowlist};
use axum::extract::State;
use axum::http::StatusCode;
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 进程内受托标记：仅由 A 侧隧道端插入请求扩展（HTTP 客户端无法伪造），
/// 鉴权中间件视为 viewer 只读（api/mod.rs auth）
#[derive(Clone, Copy)]
pub struct TunneledViewer;

/// 单候选连接超时（协议约定）
pub const CANDIDATE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct DataPlane {
    endpoint: quinn::Endpoint,
    allowlist: Arc<PeerAllowlist>,
    /// A 侧活跃连接按对端指纹归档：会话取消/断开时按指纹断开对应连接
    a_conns: Arc<Mutex<HashMap<[u8; 32], Vec<quinn::Connection>>>>,
}

impl DataPlane {
    /// 绑定 QUIC UDP 端口并拉起 A 侧 accept 循环（router 为 daemon API 路由的克隆）
    pub fn bind(
        identity: &Identity,
        quic_port: u16,
        router: axum::Router,
    ) -> Result<Arc<DataPlane>, String> {
        let allowlist = Arc::new(PeerAllowlist::default());
        let server_config = tls::server_config(identity, allowlist.clone())?;
        let endpoint = bind_endpoint(
            |addr| quinn::Endpoint::server(server_config.clone(), addr),
            quic_port,
        )?;
        let plane = Arc::new(DataPlane {
            endpoint,
            allowlist,
            a_conns: Arc::new(Mutex::new(HashMap::new())),
        });
        tokio::spawn(plane.clone().accept_loop(router));
        Ok(plane)
    }

    /// 本端 QUIC 监听地址（测试与诊断）
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }

    /// 放行对端指纹（session.request 接受时调用）
    pub fn allow_peer(&self, fingerprint: [u8; 32]) {
        self.allowlist.add(fingerprint);
    }

    /// 收回放行并断开其连接（session.cancelled / 会话结束）
    pub fn disallow_peer(&self, fingerprint: &[u8; 32]) {
        self.allowlist.remove(fingerprint);
        if let Some(conns) = self.a_conns.lock().unwrap().remove(fingerprint) {
            for conn in conns {
                conn.close(0u32.into(), b"session ended");
            }
        }
    }

    /// 通道重连时重置：旧会话的放行与连接全部失效（B 侧会感知断开并自行收尾）
    pub fn reset_peers(&self) {
        self.allowlist.clear();
        let conns = std::mem::take(&mut *self.a_conns.lock().unwrap());
        for (_, list) in conns {
            for conn in list {
                conn.close(0u32.into(), b"signal reconnect");
            }
        }
    }

    /// A 侧 accept 循环：每条连接归档对端指纹，每条双向流按 HTTP/1.1 连接服务
    async fn accept_loop(self: Arc<Self>, router: axum::Router) {
        while let Some(incoming) = self.endpoint.accept().await {
            let Ok(connecting) = incoming.accept() else {
                continue;
            };
            let plane = self.clone();
            let router = router.clone();
            tokio::spawn(async move {
                let Ok(conn) = connecting.await else {
                    return;
                };
                let peer = peer_fingerprint(&conn);
                if let Some(fp) = peer {
                    plane
                        .a_conns
                        .lock()
                        .unwrap()
                        .entry(fp)
                        .or_default()
                        .push(conn.clone());
                }
                while let Ok((send, recv)) = conn.accept_bi().await {
                    tokio::spawn(serve_http(router.clone(), recv, send));
                }
                // 连接关闭：从归档中移除
                if let Some(fp) = peer {
                    if let Some(list) = plane.a_conns.lock().unwrap().get_mut(&fp) {
                        list.retain(|c| c.stable_id() != conn.stable_id());
                    }
                }
            });
        }
    }

    /// B 侧：依候选地址连接 A 侧（钉扎指纹）。返回的 endpoint 须随连接一同持有保活
    pub async fn connect(
        identity: &Identity,
        host: &str,
        port: u16,
        expected: [u8; 32],
    ) -> Result<(quinn::Endpoint, quinn::Connection), String> {
        let ip: IpAddr = host.parse().map_err(|_| format!("候选地址非法: {host}"))?;
        let client_config = tls::client_config(identity, expected)?;
        let mut endpoint = bind_endpoint(quinn::Endpoint::client, 0)?;
        endpoint.set_default_client_config(client_config);
        let connecting = endpoint
            .connect(SocketAddr::new(ip, port), "hawk-remote")
            .map_err(|e| format!("隧道连接发起失败: {e}"))?;
        let conn = tokio::time::timeout(CANDIDATE_TIMEOUT, connecting)
            .await
            .map_err(|_| format!("候选 {host}:{port} 连接超时"))?
            .map_err(|e| format!("候选 {host}:{port} 连接失败: {e}"))?;
        Ok((endpoint, conn))
    }
}

/// QUIC UDP 绑定：优先双栈 [::]，失败回退 IPv4
fn bind_endpoint<T>(f: impl Fn(SocketAddr) -> std::io::Result<T>, port: u16) -> Result<T, String> {
    let v6 = SocketAddr::from(([0u16; 8], port));
    f(v6)
        .or_else(|_| f(SocketAddr::from(([0, 0, 0, 0], port))))
        .map_err(|e| format!("QUIC 端口 {port} 绑定失败: {e}"))
}

fn peer_fingerprint(conn: &quinn::Connection) -> Option<[u8; 32]> {
    let certs = conn
        .peer_identity()?
        .downcast::<Vec<quinn::rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    Some(tls::fingerprint_of(certs.first()?.as_ref()))
}

/// A 侧隧道端：一条双向流 = 一条 HTTP/1.1 连接，进程内转发给 API 路由
async fn serve_http(router: axum::Router, recv: quinn::RecvStream, send: quinn::SendStream) {
    let io = TokioIo::new(tokio::io::join(recv, send));
    let service =
        hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
            let router = router.clone();
            async move {
                // 隧道端拒绝转发 token 发现端点（协议安全模型：免鉴权返回主 token，只服务本机扩展）
                if req.uri().path() == "/api/v1/app/token" {
                    return Ok::<_, Infallible>(
                        axum::http::Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(axum::body::Body::empty())
                            .unwrap(),
                    );
                }
                // 进程内受托标记：鉴权中间件视为 viewer 只读
                req.extensions_mut().insert(TunneledViewer);
                let (parts, body) = req.into_parts();
                let req = axum::http::Request::from_parts(parts, axum::body::Body::new(body));
                tower::ServiceExt::oneshot(router, req).await
            }
        });
    if let Err(e) = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, service)
        .await
    {
        tracing::debug!("隧道 HTTP 连接结束: {e}");
    }
}

// ---------- B 侧会话代理 ----------

pub struct ProxyContext {
    pub conn: quinn::Connection,
    pub token: String,
}

/// 启动会话代理：环回 HTTP 监听，token 门禁，逐请求开 QUIC 流转发。
/// 代理响应剥掉 CORS 头（协议安全模型：远程查看不放开跨源）
pub async fn serve_proxy(ctx: Arc<ProxyContext>, listener: tokio::net::TcpListener) {
    let app = axum::Router::new().fallback(proxy_handler).with_state(ctx);
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("会话代理退出: {e}");
    }
}

async fn proxy_handler(
    State(ctx): State<Arc<ProxyContext>>,
    req: axum::extract::Request,
) -> Result<axum::response::Response, StatusCode> {
    if !token_valid(&req, &ctx.token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let (send, recv) = ctx.conn.open_bi().await.map_err(|e| {
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
    let mut resp = sender
        .send_request(req)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    // 剥掉 A 侧路由 CORS 层放开头（代理响应不带 CORS，协议安全模型）
    resp.headers_mut().remove("access-control-allow-origin");
    resp.headers_mut().remove("access-control-allow-headers");
    resp.headers_mut().remove("access-control-allow-methods");
    resp.headers_mut().remove("access-control-expose-headers");
    let (parts, body) = resp.into_parts();
    Ok(axum::response::Response::from_parts(
        parts,
        axum::body::Body::new(body),
    ))
}

/// 会话 token 校验：Bearer 头或查询参数（events/thumbnail/file 等 EventSource、<img> 场景）
fn token_valid(req: &axum::extract::Request, token: &str) -> bool {
    let bearer = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if let Some(value) = bearer {
        return value == token;
    }
    req.uri()
        .query()
        .and_then(|q| {
            q.split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(k, _)| *k == "token")
                .map(|(_, v)| v)
        })
        .is_some_and(|value| value == token)
}

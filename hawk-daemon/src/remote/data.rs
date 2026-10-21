//! QUIC 数据面：A 侧隧道端 + B 侧会话代理。
//! A 侧：接受 QUIC 连接，每条双向流即一条 HTTP/1.1 连接（hyper 解析），进程内转发给
//! API 路由并标记 TunneledPeer（按会话读写权限）；B 侧：环回 HTTP 代理（会话 token 门禁），
//! 每个请求开一条 QUIC 流转发。HTTP 解析两端的流式语义（SSE/缩略图）由 hyper 原生承载。

use crate::remote::identity::Identity;
use crate::remote::tls::{self, PeerAllowlist};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 隧道内会话标识头：B 侧代理写入，A 侧校验并剥离；公网 HTTP 客户端无法伪造进程内标记
pub const SESSION_HEADER: &str = "x-hawk-session";

/// 进程内受托标记：仅由 A 侧隧道端插入请求扩展（HTTP 客户端无法伪造），
/// 鉴权中间件按 writable 决定 viewer 只读或受限远程写
#[derive(Clone, Copy)]
pub struct TunneledPeer {
    pub writable: bool,
}

/// 单候选连接超时（协议约定）
pub const CANDIDATE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
struct AcceptedSession {
    fingerprint: [u8; 32],
    writable: bool,
}

/// 会话注册表：session_id → 权限，以及 QUIC 连接与会话的绑定。
/// hawk-server 的网页版与插件共用同一个设备指纹，取消单个会话不能误伤其他会话。
#[derive(Clone, Default)]
struct SessionRegistry {
    accepted: Arc<Mutex<HashMap<String, AcceptedSession>>>,
    conn_sessions: Arc<Mutex<HashMap<usize, String>>>,
    session_conns: Arc<Mutex<HashMap<String, Vec<usize>>>>,
    notify: Arc<Mutex<HashMap<String, Arc<tokio::sync::Notify>>>>,
}

impl SessionRegistry {
    /// 登记会话；返回同一 session_id 的旧条目（重复推送时避免指纹放行计数泄漏）
    fn allow(
        &self,
        session_id: &str,
        fingerprint: [u8; 32],
        writable: bool,
    ) -> Option<AcceptedSession> {
        let previous = self.accepted.lock().unwrap().insert(
            session_id.to_string(),
            AcceptedSession {
                fingerprint,
                writable,
            },
        );
        self.notify
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default();
        previous
    }

    /// 移除会话及其连接，返回 (对端指纹, 连接 id 列表)
    fn disallow(&self, session_id: &str) -> Option<([u8; 32], Vec<usize>)> {
        let session = self.accepted.lock().unwrap().remove(session_id)?;
        self.notify.lock().unwrap().remove(session_id);
        let ids = self
            .session_conns
            .lock()
            .unwrap()
            .remove(session_id)
            .unwrap_or_default();
        let mut conn_sessions = self.conn_sessions.lock().unwrap();
        for id in &ids {
            conn_sessions.remove(id);
        }
        Some((session.fingerprint, ids))
    }

    /// 校验并绑定：连接首次请求的会话必须与 QUIC 对端指纹一致；一条连接只属于一个会话
    fn bind(
        &self,
        conn_id: usize,
        session_id: &str,
        peer: Option<[u8; 32]>,
    ) -> Result<bool, StatusCode> {
        let session = self
            .accepted
            .lock()
            .unwrap()
            .get(session_id)
            .copied()
            .ok_or(StatusCode::UNAUTHORIZED)?;
        if peer != Some(session.fingerprint) {
            return Err(StatusCode::FORBIDDEN);
        }
        let mut conn_sessions = self.conn_sessions.lock().unwrap();
        match conn_sessions.get(&conn_id) {
            Some(bound) if bound != session_id => return Err(StatusCode::FORBIDDEN),
            Some(_) => {}
            None => {
                conn_sessions.insert(conn_id, session_id.to_string());
                self.session_conns
                    .lock()
                    .unwrap()
                    .entry(session_id.to_string())
                    .or_default()
                    .push(conn_id);
            }
        }
        Ok(session.writable)
    }

    fn forget_conn(&self, conn_id: usize) {
        let session_id = self.conn_sessions.lock().unwrap().remove(&conn_id);
        if let Some(session_id) = session_id {
            if let Some(ids) = self.session_conns.lock().unwrap().get_mut(&session_id) {
                ids.retain(|id| *id != conn_id);
            }
        }
    }

    async fn await_first_conn(&self, session_id: &str, timeout: Duration) -> bool {
        let notify = self
            .notify
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default()
            .clone();
        tokio::time::timeout(timeout, notify.notified())
            .await
            .is_ok()
    }

    fn reset(&self) {
        self.accepted.lock().unwrap().clear();
        self.conn_sessions.lock().unwrap().clear();
        self.session_conns.lock().unwrap().clear();
        self.notify.lock().unwrap().clear();
    }
}

pub struct DataPlane {
    /// Option：热重启/停用时显式 take 并 drop，避免超时清理等任务持有 Arc 时端口不释放
    endpoint: Mutex<Option<quinn::Endpoint>>,
    pub(crate) allowlist: Arc<PeerAllowlist>,
    /// 连接唯一 id → 连接：直连与多条中继端点共用同一注册表，不能依赖 single-endpoint 的 stable_id
    conn_by_id: Arc<Mutex<HashMap<usize, quinn::Connection>>>,
    conn_seq: Arc<AtomicU64>,
    registry: SessionRegistry,
    /// A 侧 accept 循环任务句柄（shutdown 时中止，否则它持有的端点会让端口永不释放）
    accept_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
            endpoint: Mutex::new(Some(endpoint)),
            allowlist,
            conn_by_id: Arc::new(Mutex::new(HashMap::new())),
            conn_seq: Arc::new(AtomicU64::new(1)),
            registry: SessionRegistry::default(),
            accept_task: Mutex::new(None),
        });
        #[cfg(windows)]
        tokio::spawn(crate::remote::reachability::ensure_firewall_rule_windows(
            quic_port,
        ));
        let accept_task = tokio::spawn(plane.clone().accept_loop(router));
        *plane.accept_task.lock().unwrap() = Some(accept_task);
        Ok(plane)
    }

    /// 关停数据面：关闭端点（accept 循环随即退出）并中止循环任务，释放 QUIC UDP 端口。
    /// 配置热重启/停用 remote 必须调用——否则同一进程内再次激活会因端口仍被自己占用
    /// 而绑定失败（进程退出不需要，随进程释放）
    pub fn shutdown(&self) {
        if let Some(endpoint) = self.endpoint.lock().unwrap().take() {
            endpoint.close(0u32.into(), b"");
        }
        if let Some(task) = self.accept_task.lock().unwrap().take() {
            task.abort();
        }
    }

    /// 本端 QUIC 监听地址（测试与诊断）
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.endpoint
            .lock()
            .unwrap()
            .as_ref()
            .map(|endpoint| endpoint.local_addr())
            .unwrap_or_else(|| Err(std::io::Error::other("data plane is shut down")))
    }

    /// 放行会话对端指纹并登记读写权限（session.request 接受时调用）
    pub fn allow_session(&self, session_id: &str, fingerprint: [u8; 32], writable: bool) {
        match self.registry.allow(session_id, fingerprint, writable) {
            Some(previous) if previous.fingerprint == fingerprint => {}
            Some(previous) => {
                self.allowlist.remove(&previous.fingerprint);
                self.allowlist.add(fingerprint);
            }
            None => self.allowlist.add(fingerprint),
        }
    }

    /// 等待该会话的首个隧道请求（接受会话后的超时清理用）
    pub async fn await_first_conn(&self, session_id: &str, timeout: Duration) -> bool {
        self.registry.await_first_conn(session_id, timeout).await
    }

    /// 收回会话放行并断开其连接（session.cancelled / 会话结束）
    pub fn disallow_session(&self, session_id: &str) {
        let Some((fingerprint, ids)) = self.registry.disallow(session_id) else {
            return;
        };
        self.allowlist.remove(&fingerprint);
        self.close_connections(&ids);
    }

    /// 通道重连时重置：旧会话的放行与连接全部失效（B 侧会感知断开并自行收尾）
    pub fn reset_peers(&self) {
        self.allowlist.clear();
        self.registry.reset();
        let by_id = std::mem::take(&mut *self.conn_by_id.lock().unwrap());
        for conn in by_id.into_values() {
            conn.close(0u32.into(), b"signal reconnect");
        }
    }

    fn close_connections(&self, ids: &[usize]) {
        if ids.is_empty() {
            return;
        }
        let mut by_id = self.conn_by_id.lock().unwrap();
        for id in ids {
            if let Some(conn) = by_id.remove(id) {
                conn.close(0u32.into(), b"session ended");
            }
        }
    }

    /// A 侧 accept 循环（直连 UDP 与中继 WS 共用同一逻辑）
    async fn accept_loop(self: Arc<Self>, router: axum::Router) {
        let Some(endpoint) = self.endpoint.lock().unwrap().clone() else {
            return;
        };
        accept_connections(
            endpoint,
            router,
            self.conn_by_id.clone(),
            self.conn_seq.clone(),
            self.registry.clone(),
        )
        .await;
    }

    /// 中继端点复用同一 accept 逻辑
    pub(crate) async fn accept_over(&self, endpoint: quinn::Endpoint, router: axum::Router) {
        accept_connections(
            endpoint,
            router,
            self.conn_by_id.clone(),
            self.conn_seq.clone(),
            self.registry.clone(),
        )
        .await;
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

/// A 侧 accept 循环：每条连接归档对端指纹，每条双向流按 HTTP/1.1 连接服务
async fn accept_connections(
    endpoint: quinn::Endpoint,
    router: axum::Router,
    conn_by_id: Arc<Mutex<HashMap<usize, quinn::Connection>>>,
    conn_seq: Arc<AtomicU64>,
    registry: SessionRegistry,
) {
    while let Some(incoming) = endpoint.accept().await {
        let Ok(connecting) = incoming.accept() else {
            continue;
        };
        let router = router.clone();
        let conn_by_id = conn_by_id.clone();
        let registry = registry.clone();
        let conn_id = conn_seq.fetch_add(1, Ordering::Relaxed) as usize;
        tokio::spawn(async move {
            let Ok(conn) = connecting.await else {
                return;
            };
            let peer = peer_fingerprint(&conn);
            conn_by_id.lock().unwrap().insert(conn_id, conn.clone());
            while let Ok((send, recv)) = conn.accept_bi().await {
                tokio::spawn(serve_http(
                    router.clone(),
                    peer,
                    conn_id,
                    registry.clone(),
                    recv,
                    send,
                ));
            }
            // 连接关闭：从会话绑定与连接表中移除
            registry.forget_conn(conn_id);
            conn_by_id.lock().unwrap().remove(&conn_id);
        });
    }
}

fn peer_fingerprint(conn: &quinn::Connection) -> Option<[u8; 32]> {
    let certs = conn
        .peer_identity()?
        .downcast::<Vec<quinn::rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    Some(tls::fingerprint_of(certs.first()?.as_ref()))
}

fn tunnel_response(status: StatusCode) -> axum::response::Response {
    axum::http::Response::builder()
        .status(status)
        .body(axum::body::Body::empty())
        .unwrap()
}

/// A 侧隧道端：一条双向流 = 一条 HTTP/1.1 连接，进程内转发给 API 路由
async fn serve_http(
    router: axum::Router,
    peer: Option<[u8; 32]>,
    conn_id: usize,
    registry: SessionRegistry,
    recv: quinn::RecvStream,
    send: quinn::SendStream,
) {
    let io = TokioIo::new(tokio::io::join(recv, send));
    let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let router = router.clone();
        let registry = registry.clone();
        async move {
            // 隧道端拒绝转发 token 发现端点（协议安全模型：免鉴权返回主 token，只服务本机扩展）
            if req.uri().path() == "/api/v1/app/token" {
                return Ok::<_, Infallible>(tunnel_response(StatusCode::NOT_FOUND));
            }
            let session_id = req
                .headers()
                .get(SESSION_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(String::from);
            let Some(session_id) = session_id else {
                return Ok(tunnel_response(StatusCode::UNAUTHORIZED));
            };
            let writable = match registry.bind(conn_id, &session_id, peer) {
                Ok(writable) => writable,
                Err(status) => return Ok(tunnel_response(status)),
            };
            // 首连通知：会话接受后的超时清理据此判断对端是否已接入
            if let Some(notify) = registry.notify.lock().unwrap().get(&session_id) {
                notify.notify_one();
            }
            let (mut parts, body) = req.into_parts();
            parts.headers.remove(SESSION_HEADER);
            // 进程内受托标记：鉴权中间件按 writable 决定只读或受限远程写
            parts.extensions.insert(TunneledPeer { writable });
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
    /// 隧道会话 id：每个转发请求带上，A 侧据此判定读写权限
    pub session_id: String,
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
    // 客户端自带的会话头一律丢弃，改用服务端绑定的会话标识
    let (mut parts, body) = req.into_parts();
    parts.headers.remove(SESSION_HEADER);
    parts.headers.insert(
        SESSION_HEADER,
        HeaderValue::from_str(&ctx.session_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    let req = axum::extract::Request::from_parts(parts, body);

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

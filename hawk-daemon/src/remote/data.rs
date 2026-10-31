//! 数据面（iroh 隧道）：A 侧隧道端 + B 侧会话代理。
//! A 侧：接受 iroh 连接（对端身份 = EndpointId 公钥，握手即认证），按会话放行集合裁决
//! 非放行对端即断开；每条双向流即一条 HTTP/1.1 连接（hyper 解析），进程内转发给
//! API 路由并标记 TunneledPeer（按会话读写权限）；B 侧：环回 HTTP 代理（会话 token 门禁），
//! 每个请求开一条流转发。HTTP 解析两端的流式语义（SSE/缩略图）由 hyper 原生承载。
//! 打洞/中继/路径迁移/映射保活全部是 iroh 内部事务，本模块不感知路径。

use crate::core::access::Role;
use crate::remote::identity::Identity;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 隧道 ALPN（协议常量；两侧零代码共享，各自硬编码同一值）
pub const HAWK_ALPN: &[u8] = b"hawk/1";

/// 隧道内会话标识头：B 侧代理写入，A 侧校验并剥离；公网 HTTP 客户端无法伪造进程内标记
pub const SESSION_HEADER: &str = "x-hawk-session";

/// 进程内受托标记：仅由 A 侧隧道端插入请求扩展（HTTP 客户端无法伪造），
/// 鉴权中间件按 role 决定 viewer 只读或受限远程写
#[derive(Clone)]
pub struct TunnelAccess {
    /// AccessKey 的 SHA-256 验证子；仅目标 daemon 使用，不发送给 hawk-server。
    pub credential: Option<String>,
    pub authenticated: bool,
}

#[derive(Clone)]
pub struct TunneledPeer {
    pub session_id: String,
    pub source_device_id: Option<String>,
    pub role: Role,
    /// Some = AccessKey rendezvous；None = 已注册 Device 会话。
    pub access: Option<TunnelAccess>,
}

/// B 侧拨号超时（协议约定 15s；iroh 内部竞速直连/中继，首字节不等打洞结果）
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

type EndpointId = iroh::EndpointId;

#[derive(Clone)]
struct AcceptedSession {
    peer: EndpointId,
    source_device_id: Option<String>,
    role: Role,
    access: Option<TunnelAccess>,
    lease_expires_at: std::time::Instant,
}

/// 会话注册表：session_id → 权限与对端身份，以及连接与会话的绑定。
/// （A 侧执法点在连接建立后：对端身份不在放行集合即断开——
/// 与握手期拒绝语义等价，代价是多完成一次握手）
#[derive(Clone, Default)]
struct SessionRegistry {
    accepted: Arc<Mutex<HashMap<String, AcceptedSession>>>,
    /// 放行中的对端身份集合（accepted 的索引；引用计数防同身份多会话误伤）
    peers: Arc<Mutex<HashMap<EndpointId, usize>>>,
    conn_sessions: Arc<Mutex<HashMap<usize, String>>>,
    session_conns: Arc<Mutex<HashMap<String, Vec<usize>>>>,
    notify: Arc<Mutex<HashMap<String, Arc<tokio::sync::Notify>>>>,
}

impl SessionRegistry {
    /// 登记 Device 会话；重复推送替换授权而不增加对端引用计数。
    fn allow(
        &self,
        session_id: &str,
        source_device_id: Option<String>,
        peer: EndpointId,
        role: Role,
        access_rendezvous: bool,
    ) {
        let access = access_rendezvous.then_some(TunnelAccess {
            credential: None,
            authenticated: false,
        });
        let previous = self.accepted.lock().unwrap().insert(
            session_id.to_string(),
            AcceptedSession {
                peer,
                source_device_id,
                role,
                access,
                lease_expires_at: std::time::Instant::now() + Duration::from_secs(15 * 60),
            },
        );
        if previous.map(|p| p.peer) != Some(peer) {
            *self.peers.lock().unwrap().entry(peer).or_default() += 1;
        }
        self.notify
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default();
    }

    fn renew(&self, session_id: &str) -> Result<(), StatusCode> {
        let mut accepted = self.accepted.lock().unwrap();
        let Some(session) = accepted.get_mut(session_id) else {
            return Err(StatusCode::UNAUTHORIZED);
        };
        if session
            .access
            .as_ref()
            .is_some_and(|access| !access.authenticated)
        {
            return Err(StatusCode::UNAUTHORIZED);
        }
        session.lease_expires_at = std::time::Instant::now() + Duration::from_secs(15 * 60);
        Ok(())
    }

    fn authenticate_access(
        &self,
        session_id: &str,
        credential: String,
        role: Role,
    ) -> Result<(), StatusCode> {
        let mut accepted = self.accepted.lock().unwrap();
        let Some(session) = accepted.get_mut(session_id) else {
            return Err(StatusCode::UNAUTHORIZED);
        };
        let Some(access) = &mut session.access else {
            return Err(StatusCode::FORBIDDEN);
        };
        access.credential = Some(credential);
        access.authenticated = true;
        session.role = role;
        Ok(())
    }

    /// 移除会话及其连接，返回 (对端身份, 连接 id 列表)
    fn disallow(&self, session_id: &str) -> Option<(EndpointId, Vec<usize>)> {
        let session = self.accepted.lock().unwrap().remove(session_id)?;
        self.notify.lock().unwrap().remove(session_id);
        {
            let mut peers = self.peers.lock().unwrap();
            if let Some(count) = peers.get_mut(&session.peer) {
                *count -= 1;
                if *count == 0 {
                    peers.remove(&session.peer);
                }
            }
        }
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
        Some((session.peer, ids))
    }

    fn is_allowed(&self, peer: EndpointId) -> bool {
        self.peers.lock().unwrap().contains_key(&peer)
    }

    /// 校验并绑定：连接首次请求的会话必须与对端身份一致；一条连接只属于一个会话
    fn bind(
        &self,
        conn_id: usize,
        session_id: &str,
        peer: Option<EndpointId>,
    ) -> Result<TunneledPeer, StatusCode> {
        let session = self
            .accepted
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .ok_or(StatusCode::UNAUTHORIZED)?;
        if peer != Some(session.peer) {
            return Err(StatusCode::FORBIDDEN);
        }
        if session.lease_expires_at <= std::time::Instant::now() {
            return Err(StatusCode::UNAUTHORIZED);
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
        Ok(TunneledPeer {
            session_id: session_id.to_string(),
            source_device_id: session.source_device_id,
            role: session.role,
            access: session.access,
        })
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
        self.peers.lock().unwrap().clear();
        self.conn_sessions.lock().unwrap().clear();
        self.session_conns.lock().unwrap().clear();
        self.notify.lock().unwrap().clear();
    }
}

pub struct DataPlane {
    endpoint: Mutex<Option<iroh::Endpoint>>,
    registry: SessionRegistry,
    /// 连接唯一 id → 连接（多条连接共用同一 endpoint，不能用 stable_id）
    conn_by_id: Arc<Mutex<HashMap<usize, iroh::endpoint::Connection>>>,
    conn_seq: Arc<std::sync::atomic::AtomicU64>,
    /// A 侧 accept 循环任务句柄（shutdown 时中止）
    accept_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl DataPlane {
    /// 绑定 iroh endpoint 并拉起 A 侧 accept 循环（router 为 daemon API 路由的克隆）。
    /// relay：Some((url, token)) 使用服务端内嵌中继（打洞协调 + 兜底），None 纯直连
    pub async fn bind(
        identity: &Identity,
        quic_port: u16,
        relay: Option<(String, String)>,
        router: axum::Router,
    ) -> Result<Arc<DataPlane>, String> {
        let secret_key = identity.secret_key()?;
        let relay_mode = match relay {
            Some((url, token)) => {
                let relay_url: iroh::RelayUrl = url.parse().map_err(|_| "relay url 非法")?;
                let map = iroh::RelayMap::from(relay_url).with_auth_token(token);
                iroh::RelayMode::Custom(map)
            }
            None => iroh::RelayMode::Disabled,
        };
        let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .alpns(vec![HAWK_ALPN.to_vec()])
            .secret_key(secret_key)
            .relay_mode(relay_mode);
        if quic_port != 0 {
            builder = builder
                .bind_addr(std::net::SocketAddrV4::new(
                    std::net::Ipv4Addr::UNSPECIFIED,
                    quic_port,
                ))
                .map_err(|e| format!("隧道端口配置非法: {e}"))?;
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|e| format!("隧道端口 {quic_port} 绑定失败: {e}"))?;
        let plane = Arc::new(DataPlane {
            endpoint: Mutex::new(Some(endpoint)),
            registry: SessionRegistry::default(),
            conn_by_id: Arc::new(Mutex::new(HashMap::new())),
            conn_seq: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            accept_task: Mutex::new(None),
        });
        #[cfg(windows)]
        tokio::spawn(ensure_firewall_rule_windows(quic_port));
        let accept_task = tokio::spawn(plane.clone().accept_loop(router));
        *plane.accept_task.lock().unwrap() = Some(accept_task);
        Ok(plane)
    }

    /// 关停数据面（配置热重启/停用必须调用）：关 accept 循环并关闭 endpoint
    pub fn shutdown(&self) {
        if let Some(task) = self.accept_task.lock().unwrap().take() {
            task.abort();
        }
        if let Some(endpoint) = self.endpoint.lock().unwrap().take() {
            tokio::spawn(async move {
                endpoint.close().await;
            });
        }
    }

    /// 本端 endpoint 克隆（B 侧拨号与心跳上报直连地址共用；endpoint 是句柄，克隆廉价）
    pub fn endpoint(&self) -> Option<iroh::Endpoint> {
        self.endpoint.lock().unwrap().clone()
    }

    /// 当前直连地址集（心跳上报；iroh 随网络环境刷新）
    pub fn direct_addrs(&self) -> Vec<String> {
        self.endpoint
            .lock()
            .unwrap()
            .as_ref()
            .map(|e| {
                e.addr()
                    .addrs
                    .iter()
                    .filter_map(|t| match t {
                        iroh::TransportAddr::Ip(addr) => Some(addr.to_string()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 放行 Device 会话对端身份并登记权限。
    pub fn allow_session(
        &self,
        session_id: &str,
        source_device_id: &str,
        peer: EndpointId,
        role: Role,
    ) {
        self.registry.allow(
            session_id,
            Some(source_device_id.to_string()),
            peer,
            role,
            false,
        );
    }

    /// 放行 AccessKey rendezvous；完成隧道内登录前不授予素材能力。
    pub fn allow_access_session(&self, session_id: &str, peer: EndpointId) {
        self.registry
            .allow(session_id, None, peer, Role::Viewer, true);
    }

    pub fn authenticate_access_session(
        &self,
        session_id: &str,
        credential: String,
        role: Role,
    ) -> Result<(), StatusCode> {
        self.registry
            .authenticate_access(session_id, credential, role)
    }

    pub fn renew_session(&self, session_id: &str) -> Result<(), StatusCode> {
        self.registry.renew(session_id)
    }

    /// 等待该会话的首个隧道请求（接受会话后的超时清理用）
    pub async fn await_first_conn(&self, session_id: &str, timeout: Duration) -> bool {
        self.registry.await_first_conn(session_id, timeout).await
    }

    /// 收回会话放行并断开其连接（session.cancelled / 会话结束）
    pub fn disallow_session(&self, session_id: &str) {
        let Some((_, ids)) = self.registry.disallow(session_id) else {
            return;
        };
        if ids.is_empty() {
            return;
        }
        let mut by_id = self.conn_by_id.lock().unwrap();
        for id in ids {
            if let Some(conn) = by_id.remove(&id) {
                conn.close(0u32.into(), b"session ended");
            }
        }
    }

    /// 通道重连时重置：旧会话的放行与连接全部失效（B 侧会感知断开并自行收尾）
    pub fn reset_peers(&self) {
        self.registry.reset();
        let by_id = std::mem::take(&mut *self.conn_by_id.lock().unwrap());
        for conn in by_id.into_values() {
            conn.close(0u32.into(), b"signal reconnect");
        }
    }

    /// A 侧 accept 循环：非放行对端建立连接后立即断开（执法点）
    async fn accept_loop(self: Arc<Self>, router: axum::Router) {
        let Some(endpoint) = self.endpoint.lock().unwrap().clone() else {
            return;
        };
        while let Some(incoming) = endpoint.accept().await {
            let Ok(conn) = incoming.await else {
                continue;
            };
            let peer = conn.remote_id();
            if !self.registry.is_allowed(peer) {
                tracing::debug!("未放行对端 {peer} 的连接，断开");
                conn.close(0u32.into(), b"not allowed");
                continue;
            }
            let router = router.clone();
            let registry = self.registry.clone();
            let conn_by_id = self.conn_by_id.clone();
            let conn_id =
                self.conn_seq
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed) as usize;
            conn_by_id.lock().unwrap().insert(conn_id, conn.clone());
            tokio::spawn(async move {
                while let Ok((send, recv)) = conn.accept_bi().await {
                    tokio::spawn(serve_http(
                        router.clone(),
                        Some(peer),
                        conn_id,
                        registry.clone(),
                        recv,
                        send,
                    ));
                }
                registry.forget_conn(conn_id);
                conn_by_id.lock().unwrap().remove(&conn_id);
            });
        }
    }
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
    peer: Option<EndpointId>,
    conn_id: usize,
    registry: SessionRegistry,
    recv: iroh::endpoint::RecvStream,
    send: iroh::endpoint::SendStream,
) {
    let io = TokioIo::new(tokio::io::join(recv, send));
    let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let router = router.clone();
        let registry = registry.clone();
        async move {
            // 隧道端拒绝转发 token 发现端点（协议安全模型：免鉴权返回主 token，只服务本机扩展）
            if req.uri().path() == "/api/v1/app/admin-token" {
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
            let tunnel = match registry.bind(conn_id, &session_id, peer) {
                Ok(tunnel) => tunnel,
                Err(status) => return Ok(tunnel_response(status)),
            };
            // 首连通知：会话接受后的超时清理据此判断对端是否已接入
            if let Some(notify) = registry.notify.lock().unwrap().get(&session_id) {
                notify.notify_one();
            }
            let (mut parts, body) = req.into_parts();
            parts.headers.remove(SESSION_HEADER);
            // 进程内受托标记：鉴权中间件按 Session 和当前 Grant 逐请求授权
            parts.extensions.insert(tunnel);
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
    /// 当前隧道连接（连接级重建会热替换；与 SessionManager 共享单元格）
    pub conn: Arc<std::sync::RwLock<iroh::endpoint::Connection>>,
    pub session_token: String,
    /// 隧道会话 id：每个转发请求带上，A 侧据此判定读写权限
    pub session_id: String,
    /// 当前角色：AccessKey 登录后可从 viewer 升级，后续仍由目标逐请求执法。
    pub role: std::sync::RwLock<Role>,
}

/// 启动会话代理：环回 HTTP 监听，token 门禁，逐请求开流转发。
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
    if !credential_valid(&req, &ctx.session_token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    // AccessKey 登录必须先于素材只读白名单；其余请求沿用只读检查。
    if req.uri().path() != crate::api::remote::ACCESS_SESSION_PATH
        && *ctx.role.read().unwrap() == Role::Viewer
        && !crate::api::is_viewer_allowed(&req)
    {
        return Err(StatusCode::FORBIDDEN);
    }
    // 客户端自带的会话头一律丢弃，改用服务端绑定的会话标识
    let (mut parts, body) = req.into_parts();
    parts.headers.remove(SESSION_HEADER);
    parts.headers.insert(
        SESSION_HEADER,
        HeaderValue::from_str(&ctx.session_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    );
    let req = axum::extract::Request::from_parts(parts, body);

    let conn = ctx.conn.read().unwrap().clone();
    let (send, recv) = conn.open_bi().await.map_err(|e| {
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

/// 会话 token 校验：仅 Authorization Bearer，不接受 URL 查询参数
fn credential_valid(req: &axum::extract::Request, credential: &str) -> bool {
    let bearer = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    bearer.is_some_and(|value| value == credential)
}

/// Windows 防火墙入站规则（首次启用隧道监听时添加，幂等）：无规则时入站 UDP 会被静默丢弃。
/// 规则名带端口（多库错开端口时各有一条）
#[cfg(windows)]
async fn ensure_firewall_rule_windows(quic_port: u16) {
    let rule_name = format!("hawk-remote QUIC {quic_port}");
    let rule = rule_name.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let exists = std::process::Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "show",
                "rule",
                &format!("name={rule}"),
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if exists {
            return true;
        }
        std::process::Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                &format!("name={rule}"),
                "dir=in",
                "action=allow",
                "protocol=UDP",
                &format!("localport={quic_port}"),
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
    .await;
    match outcome {
        Ok(true) => tracing::info!("Windows 防火墙入站规则就绪: {rule_name}"),
        _ => tracing::warn!(
            "Windows 防火墙入站规则添加失败（隧道入站可能被拦截，可手动放行 UDP {quic_port}）"
        ),
    }
}

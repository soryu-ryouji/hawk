//! QUIC 数据面：A 侧隧道端 + B 侧会话代理。
//! A 侧：接受 QUIC 连接，每条双向流即一条 HTTP/1.1 连接（hyper 解析），进程内转发给
//! API 路由并标记 TunneledPeer（按会话读写权限）；B 侧：环回 HTTP 代理（会话 token 门禁），
//! 每个请求开一条 QUIC 流转发。HTTP 解析两端的流式语义（SSE/缩略图）由 hyper 原生承载。

use crate::remote::identity::Identity;
use crate::remote::mux::MuxSocket;
use crate::remote::tls::{self, PeerAllowlist};
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use hyper_util::rt::TokioIo;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
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

/// 打洞候选超时（P3）：开口依赖双方时序，QUIC Initial 自动重传覆盖洞口开启前的丢包
pub const PUNCH_TIMEOUT: Duration = Duration::from_secs(15);
/// A 侧开口窗口：覆盖 B 侧打洞候选超时 + ready 推送时序差
pub const PUNCH_WINDOW: Duration = Duration::from_secs(20);
/// 开口包间隔
const PUNCH_INTERVAL: Duration = Duration::from_millis(500);
/// 开口垃圾包：仅触发本端 NAT 开口，不承载语义（对端 quinn 收到无法解析的数据报自然丢弃）
const PUNCH_PACKET: [u8; 16] = *b"HAWKPUNCH.......";

pub(crate) fn runtime() -> Result<Arc<dyn quinn::Runtime>, String> {
    quinn::default_runtime().ok_or_else(|| "tokio runtime 不可用".to_string())
}

/// 探测包（协议 P3：定长 64B = magic 5B + token 16B + 零填充）
pub(crate) fn probe_packet(token: &[u8; 16]) -> [u8; 64] {
    let mut packet = [0u8; 64];
    packet[..5].copy_from_slice(b"HAWKP");
    packet[5..21].copy_from_slice(token);
    packet
}

/// 打洞/探测发送的目标地址换算：双栈 v6 socket 不能直接发 v4 地址（EINVAL），须转为
/// v4-mapped；v4-only socket（v6 绑定失败的回退）保持原样
pub(crate) fn punch_target(local: &std::net::UdpSocket, addr: SocketAddr) -> SocketAddr {
    match (local.local_addr(), addr) {
        (Ok(SocketAddr::V6(_)), SocketAddr::V4(v4)) => {
            SocketAddr::from((v4.ip().to_ipv6_mapped(), v4.port()))
        }
        _ => addr,
    }
}

/// 探测 3 连发（UDP 单包丢失容忍；服务端收到任一即记录 observed endpoint，响应不消费）
pub(crate) async fn send_probe_packets(
    socket: &std::net::UdpSocket,
    probe_addr: SocketAddr,
    token: [u8; 16],
) {
    let probe_addr = punch_target(socket, probe_addr);
    for attempt in 0..3 {
        let _ = socket.send_to(&probe_packet(&token), probe_addr);
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
}

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
    /// 统一传输 socket（P4）：UDP 直连/打洞与中继 WS 路径合并为单一 endpoint，
    /// 路径迁移对 QUIC 连接不可见（对端换路径由 quinn 被动迁移跟随）
    mux: Mutex<Option<Arc<MuxSocket>>>,
    /// 开口句柄（P3）：与 QUIC 端点同一 UDP socket 的克隆，只发不收（接收归 mux 泵）。
    /// 探测/开口包必须与端点同端口——cone NAT 的映射按本地端口建立，换端口则服务端观测作废
    punch_socket: Mutex<Option<Arc<std::net::UdpSocket>>>,
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
        // 自建 socket + try_clone 开口句柄（P3），mux 托管读路径（P4）
        let (socket, punch_socket) = bind_udp_socket(quic_port)?;
        let mux = MuxSocket::new_server(socket);
        let endpoint = quinn::Endpoint::new_with_abstract_socket(
            quinn::EndpointConfig::default(),
            Some(server_config),
            mux.clone(),
            runtime()?,
        )
        .map_err(|e| format!("QUIC 端点创建失败: {e}"))?;
        let plane = Arc::new(DataPlane {
            endpoint: Mutex::new(Some(endpoint)),
            mux: Mutex::new(Some(mux)),
            punch_socket: Mutex::new(Some(punch_socket)),
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
        if let Some(mux) = self.mux.lock().unwrap().take() {
            mux.close();
        }
        self.punch_socket.lock().unwrap().take();
        if let Some(task) = self.accept_task.lock().unwrap().take() {
            task.abort();
        }
    }

    /// 附加中继路径（P4）：中继 WS 并入 mux——A 侧接受 loop 无需感知路径来源
    pub fn attach_relay(&self, ws: crate::remote::relay::RelayWs) {
        if let Some(mux) = self.mux.lock().unwrap().clone() {
            mux.add_relay_path(ws);
        }
    }

    /// A 侧开口（P3）：向对端 observed 地址周期发垃圾包打通本端 NAT（endpoint-dependent
    /// filtering 要求本端先向对端发包）；窗口长度覆盖 B 侧打洞候选超时与推送时序差
    pub fn punch_to(&self, peer: SocketAddr, window: Duration) {
        let Some(socket) = self.punch_socket.lock().unwrap().clone() else {
            return;
        };
        tokio::spawn(async move {
            let peer = punch_target(&socket, peer);
            let deadline = std::time::Instant::now() + window;
            while std::time::Instant::now() < deadline {
                // 非阻塞 socket 的 WouldBlock（发送缓冲压力）直接丢弃：垃圾包无需可靠
                let _ = socket.send_to(&PUNCH_PACKET, peer);
                tokio::time::sleep(PUNCH_INTERVAL).await;
            }
        });
    }

    /// 向服务端探测口报到（P3）：从 QUIC 端点同端口发出，服务端据此观测本端 NAT 映射
    pub fn send_probe(&self, probe_addr: SocketAddr, token: [u8; 16]) {
        let Some(socket) = self.punch_socket.lock().unwrap().clone() else {
            return;
        };
        tokio::spawn(async move { send_probe_packets(&socket, probe_addr, token).await });
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

    /// A 侧 accept 循环（直连 UDP 与中继 WS 路径经 mux 收口，共用同一逻辑）
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
}

/// B 侧连接（P4 mux 客户端形态）：endpoint 建在调用方预建的 mux 上（候选洪泛/中继路径
/// 由调用方在握手前配好；mux 内 socket 须已设非阻塞）；endpoint 与 mux 须随连接持有保活
pub async fn connect_mux(
    identity: &Identity,
    mux: Arc<MuxSocket>,
    expected: [u8; 32],
    timeout: Duration,
) -> Result<(quinn::Endpoint, quinn::Connection), String> {
    let mut endpoint = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        None,
        mux,
        runtime()?,
    )
    .map_err(|e| format!("端点创建失败: {e}"))?;
    endpoint.set_default_client_config(tls::client_config(identity, expected)?);
    let connecting = endpoint
        .connect(crate::remote::mux::MUX_PEER, "hawk-remote")
        .map_err(|e| format!("隧道连接发起失败: {e}"))?;
    let conn = tokio::time::timeout(timeout, connecting)
        .await
        .map_err(|_| format!("隧道握手超时（{timeout:?}）"))?
        .map_err(|e| format!("隧道握手失败: {e}"))?;
    Ok((endpoint, conn))
}

/// 自建 UDP socket（双栈优先回退 v4）+ try_clone 开口句柄；返回的端点 socket 已设非阻塞
/// （quinn 要求），开口句柄与其共享同一底层端口（P3 打洞的 NAT 映射前提）
fn bind_udp_socket(port: u16) -> Result<(std::net::UdpSocket, Arc<std::net::UdpSocket>), String> {
    let v6 = SocketAddr::from(([0u16; 8], port));
    let socket = std::net::UdpSocket::bind(v6)
        .or_else(|_| std::net::UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], port))))
        .map_err(|e| format!("QUIC 端口 {port} 绑定失败: {e}"))?;
    let punch = socket
        .try_clone()
        .map_err(|e| format!("QUIC socket 克隆失败: {e}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|e| format!("QUIC socket 非阻塞设置失败: {e}"))?;
    Ok((socket, Arc::new(punch)))
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
    /// 当前隧道连接（P4：连接级重建会热替换；与 ActiveSession 共享单元格，读锁克隆开销可忽略）
    pub conn: Arc<std::sync::RwLock<quinn::Connection>>,
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

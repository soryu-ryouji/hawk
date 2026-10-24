//! 局域网监听 supervisor：期望态收敛（reconcile）模型。
//!
//! config.toml [web] 段变更（watcher ConfigChanged）唤醒本任务，对比期望态
//! （enabled && token 非空 → 绑定 0.0.0.0:port）与实际持有监听：
//! - 端口/开关变化 → 优雅关停旧监听（3s 排空超时强杀）→ 绑新
//! - 仅 token 变化 → 期望态不变即 no-op，连接不断（token 每请求经 current().web 校验，天然热）
//! - 绑定失败（端口占用）→ LAN 保持关闭，错误写入状态（app/info 暴露，设置面板轮询展示），
//!   不崩进程——与启动期行为一致：LAN 是附属功能，本地桌面 API 优先存活
//!
//! 状态即快照（LanStatus）：active / port / error，app/info 序列化给前端。

use super::envelope::{codes, ApiError, Envelope};
use super::{require_admin, AccessLevel, SharedState};
use crate::core::config::WebSettings;
use axum::extract::{Extension, Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::time::Instant;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// 排空在途连接的上限：超时强杀（局域网查看器可自行重连，无零中断要求）
const DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// 局域网设备发现端口（固定）：UDP 组播 + 广播。仅承载设备公告，不携带任何访问 token。
pub const DISCOVERY_PORT: u16 = 27374;
const DISCOVERY_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 72, 75);
const DISCOVERY_PROBE_TIMEOUT: Duration = Duration::from_millis(900);
/// 桌面端自动配对请求的等待窗口；超时后目标端自动丢弃，无需人工清理。
const PAIR_REQUEST_TTL: Duration = Duration::from_secs(90);
/// 待处理请求上限，避免局域网内脚本刷爆目标端提示框。
const MAX_PENDING_PAIR_REQUESTS: usize = 8;
/// 自动配对请求的状态机。
#[derive(Clone)]
enum PairRequestState {
    Pending,
    Approving,
    Approved { token: String, writable: bool },
    Denied,
}

#[derive(Clone)]
struct PairRequestTicket {
    id: String,
    client_name: String,
    requested_write: bool,
    created_at_ms: u64,
    expires_at: Instant,
    state: PairRequestState,
}

/// 单个 LAN 监听器状态快照（app/info 序列化 + 日志）
#[derive(Clone, Default)]
pub struct LanStatus {
    pub active: bool,
    pub port: Option<u16>,
    pub error: Option<String>,
}

/// 网站端口与 hawk-app 直连端口各自独立收敛。
#[derive(Clone, Default)]
pub struct LanStatuses {
    pub web: LanStatus,
    pub peer: LanStatus,
}

pub struct LanSupervisor {
    notify: tokio::sync::Notify,
    /// (收敛代数, 两个监听器状态)：每轮收敛完成（含 no-op 轮）代数 +1，
    /// PUT app/lan 据此等待「本轮」结果，避免读到 wake 之前的旧状态误判
    status: Mutex<(u64, LanStatuses)>,
    /// 桌面端自动配对请求：源端创建后等待目标端用户确认；只在内存中保存。
    pair_requests: Mutex<Vec<PairRequestTicket>>,
    /// 两个监听器与发现任务的实际持有者；后台 run 与 PUT 的直接 reconcile 共用同一把锁。
    listeners: tokio::sync::Mutex<LanListeners>,
}

impl LanSupervisor {
    pub fn new() -> Arc<LanSupervisor> {
        Arc::new(LanSupervisor {
            notify: tokio::sync::Notify::new(),
            status: Mutex::new((0, LanStatuses::default())),
            pair_requests: Mutex::new(Vec::new()),
            listeners: tokio::sync::Mutex::new(LanListeners::default()),
        })
    }

    /// 创建/复用一条待确认的自动配对请求。同一设备名已有 pending 请求时直接复用，
    /// 避免用户重复点击后目标端出现多个相同提示。
    fn create_pair_request(
        &self,
        client_name: &str,
        requested_write: bool,
    ) -> Option<(String, u64)> {
        let now = Instant::now();
        let mut requests = self.pair_requests.lock().unwrap();
        requests.retain(|request| request.expires_at > now);
        if let Some(existing) = requests.iter().find(|request| {
            matches!(request.state, PairRequestState::Pending)
                && request.client_name == client_name
                && request.requested_write == requested_write
        }) {
            return Some((
                existing.id.clone(),
                existing.expires_at.saturating_duration_since(now).as_secs(),
            ));
        }
        if requests
            .iter()
            .filter(|request| matches!(request.state, PairRequestState::Pending))
            .count()
            >= MAX_PENDING_PAIR_REQUESTS
        {
            return None;
        }
        let id = loop {
            let candidate = random_hex(16);
            if requests.iter().all(|request| request.id != candidate) {
                break candidate;
            }
        };
        requests.push(PairRequestTicket {
            id: id.clone(),
            client_name: client_name.to_string(),
            requested_write,
            created_at_ms: unix_ms_now(),
            expires_at: now + PAIR_REQUEST_TTL,
            state: PairRequestState::Pending,
        });
        Some((id, PAIR_REQUEST_TTL.as_secs()))
    }

    fn pending_pair_requests(&self) -> Vec<PairRequestTicket> {
        let now = Instant::now();
        let mut requests = self.pair_requests.lock().unwrap();
        requests.retain(|request| request.expires_at > now);
        requests
            .iter()
            .filter(|request| matches!(request.state, PairRequestState::Pending))
            .cloned()
            .collect()
    }

    /// 原子地把 pending 请求标记为 approving，返回请求信息；重复点击/并发确认只会成功一次。
    fn begin_pair_request_decision(&self, id: &str) -> Option<PairRequestTicket> {
        let now = Instant::now();
        let mut requests = self.pair_requests.lock().unwrap();
        requests.retain(|request| request.expires_at > now);
        let request = requests.iter_mut().find(|request| request.id == id)?;
        if !matches!(request.state, PairRequestState::Pending) {
            return None;
        }
        request.state = PairRequestState::Approving;
        Some(request.clone())
    }

    fn finish_pair_request(&self, id: &str, state: PairRequestState) {
        if let Some(request) = self
            .pair_requests
            .lock()
            .unwrap()
            .iter_mut()
            .find(|request| request.id == id)
        {
            request.state = state;
        }
    }

    fn pair_request_status(&self, id: &str) -> Option<(String, Option<String>, Option<bool>)> {
        let now = Instant::now();
        let mut requests = self.pair_requests.lock().unwrap();
        requests.retain(|request| request.expires_at > now);
        let request = requests.iter().find(|request| request.id == id)?;
        match &request.state {
            PairRequestState::Pending | PairRequestState::Approving => {
                Some(("pending".to_string(), None, None))
            }
            PairRequestState::Approved { token, writable } => {
                Some(("approved".to_string(), Some(token.clone()), Some(*writable)))
            }
            PairRequestState::Denied => Some(("denied".to_string(), None, None)),
        }
    }

    /// 唤醒收敛（watcher 回调同步调用；Notify 无等待者时记 permit，不丢事件）
    pub fn wake(&self) {
        self.notify.notify_one();
    }

    /// 当前两个监听器状态快照
    pub fn snapshot(&self) -> LanStatuses {
        self.status.lock().unwrap().1.clone()
    }

    /// 当前收敛代数与状态快照
    fn snapshot_epoch(&self) -> (u64, LanStatuses) {
        let guard = self.status.lock().unwrap();
        (guard.0, guard.1.clone())
    }

    fn set_web_status(&self, status: LanStatus) {
        self.status.lock().unwrap().1.web = status;
    }

    fn set_peer_status(&self, status: LanStatus) {
        self.status.lock().unwrap().1.peer = status;
    }

    #[cfg(test)]
    pub(crate) fn mark_peer_active_for_test(&self, port: u16) {
        self.set_peer_status(LanStatus {
            active: true,
            port: Some(port),
            error: None,
        });
    }

    /// 本轮收敛完成：推进代数（PUT app/lan 的等待出口）
    fn bump_epoch(&self) {
        self.status.lock().unwrap().0 += 1;
    }

    /// 立即按当前配置收敛一次；PUT /app/lan 与启动期直接调用，不依赖后台任务是否被调度。
    pub async fn reconcile_once(&self, state: &SharedState) -> LanStatuses {
        let Some(lib) = state.library.clone() else {
            return self.snapshot();
        };
        let web = lib.config.current().web;
        let desired_web = desired_web_port(&web);
        let desired_peer = desired_peer_port(&web);
        let mut listeners = self.listeners.lock().await;

        let web_status = reconcile_listener(
            state,
            super::ListenerMode::Web,
            desired_web,
            &mut listeners.web,
        )
        .await;
        self.set_web_status(web_status);

        let peer_status = reconcile_listener(
            state,
            super::ListenerMode::Peer,
            desired_peer,
            &mut listeners.peer,
        )
        .await;
        self.set_peer_status(peer_status.clone());

        let peer_ready = desired_peer.is_some() && peer_status.active;
        if peer_ready && listeners.discovery.is_none() {
            listeners.discovery = Some(tokio::spawn(run_discovery_responder(state.clone())));
        } else if !peer_ready {
            if let Some(task) = listeners.discovery.take() {
                task.abort();
            }
        }
        self.bump_epoch();
        self.snapshot()
    }

    /// 常驻任务：唤醒/周期 tick → 收敛 → 等待下一轮。
    /// 仅本地库模式由 bootstrap 拉起（远程查看模式无库内 [web] 配置）。
    pub async fn run(self: Arc<Self>, state: SharedState) {
        if state.library.is_none() {
            return;
        }
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            let _ = self.reconcile_once(&state).await;
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = ticker.tick() => {}
            }
        }
    }
}

type ListenerSlot = Option<(
    u16,
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Sender<()>,
)>;

#[derive(Default)]
struct LanListeners {
    web: ListenerSlot,
    peer: ListenerSlot,
    discovery: Option<tokio::task::JoinHandle<()>>,
}

/// 单个监听器收敛：端口变化才重绑，绑定失败写入状态但不影响另一个监听器。
async fn reconcile_listener(
    state: &SharedState,
    mode: super::ListenerMode,
    desired: Option<u16>,
    current: &mut ListenerSlot,
) -> LanStatus {
    let label = match mode {
        super::ListenerMode::Web => "局域网网站",
        super::ListenerMode::Peer => "hawk-app 直连",
        super::ListenerMode::Local => "本机",
    };
    let bound = current.as_ref().map(|(port, _, _)| *port);
    if bound == desired {
        return match desired {
            Some(port) => LanStatus {
                active: true,
                port: Some(port),
                error: None,
            },
            None => LanStatus::default(),
        };
    }

    if let Some((_, handle, shutdown)) = current.take() {
        let _ = shutdown.send(());
        if tokio::time::timeout(DRAIN_TIMEOUT, handle).await.is_err() {
            tracing::warn!("{label}旧监听 {DRAIN_TIMEOUT:?} 内未排空，强制结束");
        }
    }

    let Some(port) = desired else {
        tracing::info!("{label}监听已停止");
        return LanStatus::default();
    };
    match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
        Ok(listener) => {
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let app = crate::api::build_router_for(state.clone(), mode);
            let handle = tokio::spawn(async move {
                if let Err(e) = axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.await;
                    })
                    .await
                {
                    tracing::error!("{label}监听退出: {e}");
                }
            });
            *current = Some((port, handle, shutdown_tx));
            tracing::info!("{label}监听已启动: 0.0.0.0:{port}");
            LanStatus {
                active: true,
                port: Some(port),
                error: None,
            }
        }
        Err(e) => {
            tracing::error!("{label}端口 {port} 绑定失败: {e}");
            LanStatus {
                active: false,
                port: None,
                error: Some(format!("{label}端口 {port} 绑定失败: {e}")),
            }
        }
    }
}

// ---------- 局域网发现与配对：不经服务器的 hawk-app 互联 ----------
// 发现只回答「谁在同一网段」；token 必须在目标端用户确认后返回，不进入广播/组播。

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("生成随机数失败");
    buf.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 局域网公告：只暴露展示所需字段。token 不广播，配对成功后才由目标端返回。
#[derive(Serialize, Deserialize)]
struct DiscoveryMessage {
    hawk: u8,
    op: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    library_name: String,
    #[serde(default)]
    port: u16,
    #[serde(default)]
    writable: bool,
}

/// 本机稳定 peer id（不落盘）：主机名 + 库根路径。库内 `.hawk` 不新增文件，也避免把身份
/// 写进可被云盘同步的配置；局域网展示只需要稳定区分设备，不承担密码学身份。
fn local_peer_id(lib: &super::LibraryState) -> String {
    let host = gethostname::gethostname().to_string_lossy().to_string();
    let digest = blake3::hash(format!("{host}|{}", lib.paths.root).as_bytes());
    digest.to_hex().to_string()[..16].to_string()
}

fn local_library_name(lib: &super::LibraryState) -> String {
    lib.config
        .current()
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            std::path::Path::new(&lib.paths.root)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("hawk 素材库")
                .to_string()
        })
}

fn announce_message(lib: &super::LibraryState, port: u16, writable: bool) -> DiscoveryMessage {
    DiscoveryMessage {
        hawk: 1,
        op: "announce".to_string(),
        id: local_peer_id(lib),
        name: gethostname::gethostname().to_string_lossy().to_string(),
        library_name: local_library_name(lib),
        port,
        writable,
    }
}

/// 局域网发现响应器：收到 probe 后，仅当本库已启用 LAN web 查看时回一条公告。
/// 发现协议承载的是「有哪些设备可以配对」，真正的访问 token 必须走配对码交换。
pub async fn run_discovery_responder(state: SharedState) {
    let socket = match UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)).await {
        Ok(socket) => socket,
        Err(error) => {
            tracing::warn!("局域网发现端口 {DISCOVERY_PORT} 绑定失败（自动发现不可用）: {error}");
            return;
        }
    };
    if let Err(error) = socket.join_multicast_v4(DISCOVERY_GROUP, Ipv4Addr::UNSPECIFIED) {
        tracing::warn!("加入局域网发现组播失败（仍尝试广播）: {error}");
    }
    let _ = socket.set_multicast_loop_v4(true);
    tracing::info!("局域网发现响应器已启动: UDP {DISCOVERY_PORT}（组播 + 广播）");
    let mut buffer = [0u8; 2048];
    loop {
        let Ok((len, peer)) = socket.recv_from(&mut buffer).await else {
            continue;
        };
        let Ok(message) = serde_json::from_slice::<DiscoveryMessage>(&buffer[..len]) else {
            continue;
        };
        if message.hawk != 1 || message.op != "probe" {
            continue;
        }
        let Some(lib) = state.library.as_ref() else {
            continue;
        };
        let web = lib.config.current().web;
        // 自动配对模型下，即使还没有任何直连密钥也要应答发现：目标端用户批准后
        // 会即时创建该设备专用的 peer key。
        let reply = announce_message(lib, web.peer_port, true);
        if let Ok(payload) = serde_json::to_vec(&reply) {
            let _ = socket.send_to(&payload, peer).await;
        }
    }
}

/// 探测同网段可配对的 hawk 设备（本机 daemon 发 UDP probe，目标 daemon 单播回复）。
/// 不走服务器，也不依赖 mDNS 库：组播 + 全局广播覆盖常见家用/办公网段。
/// 默认路由对应的本机 IPv4。多网卡机器（Wi-Fi + Hyper-V/VPN）不能只 bind 0.0.0.0：
/// Windows 可能把全局广播从虚拟网卡发出，真实局域网上的设备永远收不到 probe。
fn default_route_ipv4() -> Option<Ipv4Addr> {
    // 只做 UDP connect 选路，不发送任何数据；目标不可达也不影响拿到本机出口地址。
    for target in ["192.0.2.1:80", "8.8.8.8:80", "1.1.1.1:80"] {
        let Ok(socket) = std::net::UdpSocket::bind(("0.0.0.0", 0)) else {
            continue;
        };
        if socket.connect(target).is_err() {
            continue;
        }
        if let Ok(std::net::SocketAddr::V4(addr)) = socket.local_addr() {
            let ip = *addr.ip();
            if !ip.is_unspecified() && !ip.is_loopback() {
                return Some(ip);
            }
        }
    }
    None
}

/// 发现探测目标：组播之外同时发全局广播与常见 /24 子网定向广播。
/// 组播/VLAN 隔离或 Windows 多网卡选路错误时，定向广播是必要的兜底。
/// 本机全部可用 IPv4 候选：主机名解析（Windows 会返回所有网卡）∪ 默认路由出口。
/// 过滤回环、未指定和 link-local，避免把 probe 发到无效接口。
fn local_ipv4_candidates() -> Vec<Ipv4Addr> {
    let mut candidates = Vec::new();
    if let Ok(hostname) = gethostname::gethostname().into_string() {
        if let Ok(addrs) = (hostname.as_str(), 0).to_socket_addrs() {
            for addr in addrs {
                if let SocketAddr::V4(addr) = addr {
                    let ip = *addr.ip();
                    if !ip.is_unspecified() && !ip.is_loopback() && !ip.is_link_local() {
                        candidates.push(ip);
                    }
                }
            }
        }
    }
    if let Some(ip) = default_route_ipv4() {
        candidates.push(ip);
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn discovery_targets(local_ip: Option<Ipv4Addr>) -> Vec<SocketAddrV4> {
    let mut targets = vec![
        SocketAddrV4::new(DISCOVERY_GROUP, DISCOVERY_PORT),
        SocketAddrV4::new(Ipv4Addr::BROADCAST, DISCOVERY_PORT),
    ];
    if let Some(ip) = local_ip {
        let [a, b, c, _] = ip.octets();
        let subnet_broadcast = SocketAddrV4::new(Ipv4Addr::new(a, b, c, 255), DISCOVERY_PORT);
        if !targets.contains(&subnet_broadcast) {
            targets.push(subnet_broadcast);
        }
    }
    targets
}

async fn discover_peers(state: &SharedState) -> Result<Vec<LanPeerDto>, ApiError> {
    let deadline = Instant::now() + DISCOVERY_PROBE_TIMEOUT;
    let candidates = local_ipv4_candidates();
    let bind_ips = if candidates.is_empty() {
        vec![Ipv4Addr::UNSPECIFIED]
    } else {
        candidates
    };
    let mut sockets = Vec::new();
    for ip in bind_ips {
        match UdpSocket::bind(SocketAddrV4::new(ip, 0)).await {
            Ok(socket) => {
                let _ = socket.set_broadcast(true);
                let _ = socket.set_multicast_loop_v4(true);
                sockets.push((ip, socket));
            }
            Err(error) => tracing::warn!("局域网发现网卡 {ip} 绑定失败: {error}"),
        }
    }
    if sockets.is_empty() {
        return Err(ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "局域网探测端口分配失败".to_string(),
        ));
    }
    let probe = serde_json::to_vec(&DiscoveryMessage {
        hawk: 1,
        op: "probe".to_string(),
        id: String::new(),
        name: String::new(),
        library_name: String::new(),
        port: 0,
        writable: false,
    })
    .expect("发现 probe 序列化失败");
    let bind_summary = sockets.iter().map(|(ip, _)| *ip).collect::<Vec<_>>();
    tracing::info!("局域网发现 probe: bind={bind_summary:?}");
    // UDP 无重传；每个网卡连续发送三次，降低丢包概率。组播 + 全局广播 + /24 定向广播并行覆盖。
    for attempt in 0..3 {
        for (ip, socket) in &sockets {
            for target in discovery_targets(Some(*ip)) {
                let _ = socket.send_to(&probe, target).await;
            }
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
    }

    let local_id = state
        .library
        .as_ref()
        .map(|lib| local_peer_id(lib))
        .unwrap_or_default();
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<(Vec<u8>, SocketAddr)>(64);
    for (ip, socket) in sockets {
        let sender = sender.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 2048];
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let Ok(Ok((len, peer))) =
                    tokio::time::timeout(deadline - now, socket.recv_from(&mut buffer)).await
                else {
                    break;
                };
                if sender.send((buffer[..len].to_vec(), peer)).await.is_err() {
                    break;
                }
            }
            tracing::trace!("局域网发现接收任务结束: {ip}");
        });
    }
    drop(sender);

    let mut found: std::collections::HashMap<String, LanPeerDto> = std::collections::HashMap::new();
    while let Some((data, peer)) = receiver.recv().await {
        let Ok(message) = serde_json::from_slice::<DiscoveryMessage>(&data) else {
            continue;
        };
        if message.hawk != 1
            || message.op != "announce"
            || message.id.is_empty()
            || message.id == local_id
            || message.port == 0
        {
            continue;
        }
        let host = match peer.ip() {
            std::net::IpAddr::V4(ip) => ip.to_string(),
            // 局域网发现当前只广播 IPv4；IPv6 设备留给后续 mDNS 方案
            std::net::IpAddr::V6(_) => continue,
        };
        found.insert(
            message.id.clone(),
            LanPeerDto {
                id: message.id,
                name: message.name,
                library_name: message.library_name,
                url: format!("http://{host}:{}", message.port),
                writable: message.writable,
                requires_pairing: true,
            },
        );
    }
    let mut peers: Vec<_> = found.into_values().collect();
    peers.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    if peers.is_empty() {
        tracing::info!("局域网发现未找到可配对设备（bind={bind_summary:?}）");
    } else {
        tracing::info!(
            "局域网发现 {} 台设备: {:?}",
            peers.len(),
            peers.iter().map(|peer| &peer.id).collect::<Vec<_>>()
        );
    }
    Ok(peers)
}

// ---------- app/lan 端点：LAN 查看配置的读写（admin 限定） ----------
// 配置由 daemon 权威读写（toml_edit 保留注释），保存即热重绑：写配置 → reload → wake →
// 等待本轮收敛（epoch），绑定失败回滚旧配置。不再经 Electron 主进程手写 TOML + 轮询 app/info。

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(get_lan, put_lan))
        .routes(routes!(discover_lan_peers))
        .routes(routes!(create_lan_pair_request))
        .routes(routes!(list_lan_pair_requests))
        .routes(routes!(get_lan_pair_request_status))
        .routes(routes!(decide_lan_pair_request))
}

/// 发现到的局域网 hawk 设备（不回传 token；配对成功后才保存在源端连接配置中）。
#[derive(Serialize, utoipa::ToSchema)]
struct LanPeerDto {
    id: String,
    name: String,
    library_name: String,
    url: String,
    writable: bool,
    requires_pairing: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
struct LanPeersDto {
    peers: Vec<LanPeerDto>,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct LanPairRequestCreateBody {
    /// 源端设备名（仅用于目标端确认提示）
    client_name: String,
    /// 源端希望获得写权限；最终由目标端用户决定
    #[serde(default)]
    writable: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
struct LanPairRequestCreatedDto {
    request_id: String,
    expires_in_seconds: u64,
}

#[derive(Serialize, utoipa::ToSchema)]
struct LanPairRequestDto {
    id: String,
    client_name: String,
    requested_write: bool,
    created_at_ms: u64,
}

#[derive(Serialize, utoipa::ToSchema)]
struct LanPairRequestsDto {
    requests: Vec<LanPairRequestDto>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct LanPairRequestStatusDto {
    /// pending / approved / denied / expired
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    writable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    library_name: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct LanPairDecisionBody {
    allow: bool,
    #[serde(default)]
    write: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
struct LanPairDecisionDto {
    status: String,
}

fn sanitized_client_name(raw: &str) -> String {
    let name = raw.trim();
    let name: String = if name.is_empty() {
        "未知设备".to_string()
    } else {
        name.chars().take(64).collect()
    };
    name
}

/// 源端发起自动配对请求（免 Bearer，仅 hawk-app 直连端口）。
#[utoipa::path(
    post,
    path = "/api/v1/app/lan/pair-requests",
    tags = ["app"],
    request_body = LanPairRequestCreateBody,
    responses((status = 200, description = "OK", body = Envelope<LanPairRequestCreatedDto>))
)]
async fn create_lan_pair_request(
    State(state): State<SharedState>,
    Extension(mode): Extension<super::ListenerMode>,
    Json(body): Json<LanPairRequestCreateBody>,
) -> Result<Json<Envelope<LanPairRequestCreatedDto>>, ApiError> {
    if mode != super::ListenerMode::Peer {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::NOT_FOUND,
            "配对请求仅在 hawk-app 直连端口可用",
        ));
    }
    let _ = state.require_library()?;
    if !state.lan.snapshot().peer.active {
        return Err(ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "本机 hawk-app 直连监听尚未就绪",
        ));
    }
    let client_name = sanitized_client_name(&body.client_name);
    let Some((request_id, expires_in_seconds)) =
        state.lan.create_pair_request(&client_name, body.writable)
    else {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "目标设备待确认的配对请求过多，请稍后再试",
        ));
    };
    tracing::info!("收到局域网配对请求: {client_name}（request={request_id}）");
    Ok(Json(Envelope::ok(LanPairRequestCreatedDto {
        request_id,
        expires_in_seconds,
    })))
}

/// 源端轮询配对请求结果（免 Bearer；request_id 为随机不可猜 secret）。
#[utoipa::path(
    get,
    path = "/api/v1/app/lan/pair-requests/{request_id}",
    tags = ["app"],
    params(("request_id" = String, Path, description = "配对请求 id")),
    responses((status = 200, description = "OK", body = Envelope<LanPairRequestStatusDto>))
)]
async fn get_lan_pair_request_status(
    State(state): State<SharedState>,
    Extension(mode): Extension<super::ListenerMode>,
    Path(request_id): Path<String>,
) -> Result<Json<Envelope<LanPairRequestStatusDto>>, ApiError> {
    if mode != super::ListenerMode::Peer {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::NOT_FOUND,
            "配对请求仅在 hawk-app 直连端口可用",
        ));
    }
    let lib = state.require_library()?;
    let Some((status, token, writable)) = state.lan.pair_request_status(&request_id) else {
        return Ok(Json(Envelope::ok(LanPairRequestStatusDto {
            status: "expired".to_string(),
            token: None,
            writable: None,
            peer_id: None,
            name: None,
            library_name: None,
        })));
    };
    let approved = status == "approved";
    Ok(Json(Envelope::ok(LanPairRequestStatusDto {
        status,
        token,
        writable,
        peer_id: approved.then(|| local_peer_id(lib)),
        name: approved.then(|| gethostname::gethostname().to_string_lossy().to_string()),
        library_name: approved.then(|| local_library_name(lib)),
    })))
}

/// 目标端待处理配对请求列表（仅本机 admin 设置面板轮询）。
#[utoipa::path(
    get,
    path = "/api/v1/app/lan/pair-requests",
    tags = ["app"],
    responses((status = 200, description = "OK", body = Envelope<LanPairRequestsDto>))
)]
async fn list_lan_pair_requests(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
) -> Result<Json<Envelope<LanPairRequestsDto>>, ApiError> {
    require_admin(&access)?;
    let _ = state.require_library()?;
    Ok(Json(Envelope::ok(LanPairRequestsDto {
        requests: state
            .lan
            .pending_pair_requests()
            .into_iter()
            .map(|request| LanPairRequestDto {
                id: request.id,
                client_name: request.client_name,
                requested_write: request.requested_write,
                created_at_ms: request.created_at_ms,
            })
            .collect(),
    })))
}

/// 目标端用户确认/拒绝配对请求（admin 限定）。允许时创建一把该设备专用的直连密钥。
#[utoipa::path(
    post,
    path = "/api/v1/app/lan/pair-requests/{request_id}/decision",
    tags = ["app"],
    params(("request_id" = String, Path, description = "配对请求 id")),
    request_body = LanPairDecisionBody,
    responses((status = 200, description = "OK", body = Envelope<LanPairDecisionDto>))
)]
async fn decide_lan_pair_request(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
    Path(request_id): Path<String>,
    Json(body): Json<LanPairDecisionBody>,
) -> Result<Json<Envelope<LanPairDecisionDto>>, ApiError> {
    require_admin(&access)?;
    let lib = state.require_library()?;
    let Some(request) = state.lan.begin_pair_request_decision(&request_id) else {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::CONFLICT,
            "配对请求不存在、已处理或已过期",
        ));
    };
    if !body.allow {
        state
            .lan
            .finish_pair_request(&request_id, PairRequestState::Denied);
        tracing::info!(
            "局域网配对请求已拒绝: {}（request={request_id}）",
            request.client_name
        );
        return Ok(Json(Envelope::ok(LanPairDecisionDto {
            status: "denied".to_string(),
        })));
    }
    let mut web = lib.config.current().web;
    if web.peer_keys.len() >= 32 {
        state
            .lan
            .finish_pair_request(&request_id, PairRequestState::Denied);
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "直连访问密钥已达上限，请先删除不再使用的设备",
        ));
    }
    let token = random_hex(32);
    let key_id = format!("peer_{}", random_hex(8));
    web.peer_keys.push(crate::core::config::WebAccessKey {
        id: key_id,
        label: format!("局域网设备：{}", request.client_name),
        token: token.clone(),
        write: body.write,
        created_at_ms: unix_ms_now(),
    });
    if let Err(error) = lib.config.update_web(&web) {
        state
            .lan
            .finish_pair_request(&request_id, PairRequestState::Denied);
        return Err(ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            error,
        ));
    }
    state.lan.wake();
    state.lan.finish_pair_request(
        &request_id,
        PairRequestState::Approved {
            token,
            writable: body.write,
        },
    );
    state.bus.publish(
        crate::core::events::LanEvents::ACCESS_CHANGED,
        serde_json::json!({ "changed": true }),
    );
    tracing::info!(
        "局域网配对请求已允许: {}（request={request_id}, write={}）",
        request.client_name,
        body.write
    );
    Ok(Json(Envelope::ok(LanPairDecisionDto {
        status: "approved".to_string(),
    })))
}

/// 探测局域网内可配对的 hawk 设备（admin 限定；源端 app 经本机 daemon 调用）。
#[utoipa::path(
    get,
    path = "/api/v1/app/lan/peers",
    tags = ["app"],
    responses((status = 200, description = "OK", body = Envelope<LanPeersDto>))
)]
async fn discover_lan_peers(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
) -> Result<Json<Envelope<LanPeersDto>>, ApiError> {
    require_admin(&access)?;
    Ok(Json(Envelope::ok(LanPeersDto {
        peers: discover_peers(&state).await?,
    })))
}

#[derive(Serialize, utoipa::ToSchema)]
struct LanAccessKeyDto {
    id: String,
    label: String,
    /// 完整 token 仅 admin 可读；设置面板负责打码展示与复制
    token: String,
    write: bool,
    created_at_ms: u64,
}

#[derive(Serialize, utoipa::ToSchema)]
struct LanSettingsDto {
    enabled: bool,
    port: u16,
    peer_port: u16,
    keys: Vec<LanAccessKeyDto>,
    peer_keys: Vec<LanAccessKeyDto>,
    /// 网站监听状态
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// hawk-app 直连监听状态
    peer_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    peer_error: Option<String>,
}

fn lan_dto(state: &SharedState, lib: &super::LibraryState) -> LanSettingsDto {
    let web = lib.config.current().web;
    let status = state.lan.snapshot();
    LanSettingsDto {
        enabled: web.enabled,
        port: web.port,
        peer_port: web.peer_port,
        keys: web
            .keys
            .into_iter()
            .map(|key| LanAccessKeyDto {
                id: key.id,
                label: key.label,
                token: key.token,
                write: key.write,
                created_at_ms: key.created_at_ms,
            })
            .collect(),
        peer_keys: web
            .peer_keys
            .into_iter()
            .map(|key| LanAccessKeyDto {
                id: key.id,
                label: key.label,
                token: key.token,
                write: key.write,
                created_at_ms: key.created_at_ms,
            })
            .collect(),
        active: status.web.active,
        error: status.web.error,
        peer_active: status.peer.active,
        peer_error: status.peer.error,
    }
}

/// 读取局域网 web 查看配置与运行状态（admin 限定；viewer 403，防止只读 token 经此提权）
#[utoipa::path(
    get,
    path = "/api/v1/app/lan",
    tags = ["app"],
    responses((status = 200, description = "OK", body = Envelope<LanSettingsDto>))
)]
async fn get_lan(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
) -> Result<Json<Envelope<LanSettingsDto>>, ApiError> {
    require_admin(&access)?;
    let lib = state.require_library()?;
    Ok(Json(Envelope::ok(lan_dto(&state, lib))))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct PutLanKey {
    id: String,
    label: String,
    token: String,
    #[serde(default)]
    write: bool,
    #[serde(default)]
    created_at_ms: u64,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct PutLanBody {
    enabled: bool,
    port: u16,
    peer_port: u16,
    #[serde(default)]
    keys: Vec<PutLanKey>,
    #[serde(default)]
    peer_keys: Vec<PutLanKey>,
}

/// 网站端口期望态：启用且有网站密钥。
fn desired_web_port(web: &WebSettings) -> Option<u16> {
    if web.enabled && !web.keys.is_empty() {
        Some(web.port)
    } else {
        None
    }
}

/// hawk-app 直连端口期望态：始终监听，用于接收自动配对请求；不依赖网站开关或已有密钥。
fn desired_peer_port(web: &WebSettings) -> Option<u16> {
    Some(web.peer_port)
}

/// 尝试在 preferred 起连续范围内选一个可绑定端口（跳过另一方向端口）。
async fn find_available_port(preferred: u16, exclude: u16) -> Option<u16> {
    let start = preferred.max(1);
    for port in start..=start.saturating_add(32) {
        if port == exclude {
            continue;
        }
        if tokio::net::TcpListener::bind(("0.0.0.0", port))
            .await
            .is_ok()
        {
            return Some(port);
        }
    }
    None
}

/// 等待 supervisor 完成一轮收敛并达到期望态；绑定失败返回错误原因。
/// epoch 机制保证判定基于 wake 之后的新一轮收敛，而非残留旧状态。
async fn wait_converged(state: &SharedState, web: &WebSettings, epoch0: u64) -> Result<(), String> {
    let desired_web = desired_web_port(web);
    let desired_peer = desired_peer_port(web);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (epoch, statuses) = state.lan.snapshot_epoch();
        if epoch > epoch0 {
            if let Some(err) = statuses.web.error.clone() {
                return Err(err);
            }
            if let Some(err) = statuses.peer.error.clone() {
                return Err(err);
            }
            let web_ok = match desired_web {
                Some(port) => statuses.web.active && statuses.web.port == Some(port),
                None => !statuses.web.active,
            };
            let peer_ok = match desired_peer {
                Some(port) => statuses.peer.active && statuses.peer.port == Some(port),
                None => !statuses.peer.active,
            };
            if web_ok && peer_ok {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let (_, statuses) = state.lan.snapshot_epoch();
            let detail = statuses
                .web
                .error
                .or(statuses.peer.error)
                .unwrap_or_else(|| {
                    format!(
                        "网站监听 active={} port={:?}，hawk-app 监听 active={} port={:?}",
                        statuses.web.active,
                        statuses.web.port,
                        statuses.peer.active,
                        statuses.peer.port
                    )
                });
            return Err(format!("局域网设置生效超时：{detail}"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 写回 .hawk/config.toml 的 [web] 段并热重绑局域网监听（admin 限定）；绑定失败自动回滚旧配置并返回错误
#[utoipa::path(
    put,
    path = "/api/v1/app/lan",
    tags = ["app"],
    request_body = PutLanBody,
    responses((status = 200, description = "OK", body = Envelope<LanSettingsDto>))
)]
async fn put_lan(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
    Json(body): Json<PutLanBody>,
) -> Result<Json<Envelope<LanSettingsDto>>, ApiError> {
    require_admin(&access)?;
    if body.port == 0 || body.peer_port == 0 {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "端口须为 1–65535 之间的数字",
        ));
    }
    if body.port == body.peer_port {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "网站端口与 hawk-app 直连端口不能相同",
        ));
    }
    if body.keys.len() > 32 || body.peer_keys.len() > 32 {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "同类访问密钥最多 32 把",
        ));
    }
    let mut ids = std::collections::HashSet::new();
    let mut tokens = std::collections::HashSet::new();
    let mut parse_keys =
        |input: Vec<PutLanKey>| -> Result<Vec<crate::core::config::WebAccessKey>, ApiError> {
            let mut out = Vec::with_capacity(input.len());
            for key in input {
                let id = key.id.trim().to_string();
                let token = key.token.trim().to_string();
                if id.is_empty() || token.is_empty() {
                    return Err(ApiError::new(
                        codes::INVALID_PARAM,
                        axum::http::StatusCode::BAD_REQUEST,
                        "访问密钥的 id 与 token 不能为空",
                    ));
                }
                if !ids.insert(id.clone()) || !tokens.insert(token.clone()) {
                    return Err(ApiError::new(
                        codes::INVALID_PARAM,
                        axum::http::StatusCode::BAD_REQUEST,
                        "访问密钥 id/token 不能重复",
                    ));
                }
                out.push(crate::core::config::WebAccessKey {
                    id,
                    label: key.label.trim().to_string(),
                    token,
                    write: key.write,
                    created_at_ms: key.created_at_ms,
                });
            }
            Ok(out)
        };
    let keys = parse_keys(body.keys)?;
    let peer_keys = parse_keys(body.peer_keys)?;
    if body.enabled && keys.is_empty() {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "启用局域网网站访问前请至少创建一把网站访问密钥",
        ));
    }
    // 端口被其它进程占用时自动向后选一个可用端口，避免用户生成配对码前还要手动排障。
    let current = state.lan.snapshot();
    let owned_web = current.web.active && current.web.port == Some(body.port);
    let owned_peer = current.peer.active && current.peer.port == Some(body.peer_port);
    let web_needed = body.enabled && !keys.is_empty();
    let peer_needed = true;
    let mut web_port = body.port;
    let mut peer_port = body.peer_port;

    if web_needed && !owned_web {
        let available = tokio::net::TcpListener::bind(("0.0.0.0", web_port))
            .await
            .is_ok();
        if !available {
            web_port = find_available_port(web_port, peer_port)
                .await
                .ok_or_else(|| {
                    ApiError::new(
                        codes::INVALID_PARAM,
                        axum::http::StatusCode::BAD_REQUEST,
                        "附近没有可用的局域网网站端口",
                    )
                })?;
        }
    }
    if peer_needed && !owned_peer {
        let available = tokio::net::TcpListener::bind(("0.0.0.0", peer_port))
            .await
            .is_ok();
        if !available {
            peer_port = find_available_port(peer_port, web_port)
                .await
                .ok_or_else(|| {
                    ApiError::new(
                        codes::INVALID_PARAM,
                        axum::http::StatusCode::BAD_REQUEST,
                        "附近没有可用的 hawk-app 直连端口",
                    )
                })?;
        }
    }

    let new_web = WebSettings {
        enabled: body.enabled,
        port: web_port,
        keys,
        peer_keys,
        peer_port,
    };

    let lib = state.require_library()?;
    let old_web = lib.config.current().web;
    let epoch0 = state.lan.snapshot_epoch().0;
    lib.config.update_web(&new_web).map_err(|e| {
        ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e,
        )
    })?;
    state.lan.wake();
    // 不把生效完全押在后台任务的通知上：PUT 直接做一次同步收敛。
    let _ = state.lan.reconcile_once(&state).await;

    if let Err(err) = wait_converged(&state, &new_web, epoch0).await {
        // 失败回滚：写回旧配置，走同一条热更路径收敛回旧态（尽力，结果不敏感）
        if lib.config.update_web(&old_web).is_ok() {
            let epoch1 = state.lan.snapshot_epoch().0;
            state.lan.wake();
            let _ = state.lan.reconcile_once(&state).await;
            let _ = wait_converged(&state, &old_web, epoch1).await;
        }
        return Err(ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("局域网监听未生效：{err}（已回滚原配置）"),
        ));
    }
    state.bus.publish(
        crate::core::events::LanEvents::ACCESS_CHANGED,
        serde_json::json!({ "changed": true }),
    );
    Ok(Json(Envelope::ok(lan_dto(&state, lib))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_listener_is_always_desired_for_auto_pairing() {
        let web = WebSettings {
            enabled: false,
            port: 27372,
            peer_port: 27375,
            keys: vec![],
            peer_keys: vec![],
        };
        assert_eq!(desired_peer_port(&web), Some(27375));
    }

    #[test]
    fn listener_desired_for_peer_keys_even_when_website_off() {
        let web = WebSettings {
            enabled: false,
            port: 27372,
            peer_port: 27375,
            keys: vec![],
            peer_keys: vec![crate::core::config::WebAccessKey {
                id: "peer".into(),
                label: "peer".into(),
                token: "peer-token".into(),
                write: false,
                created_at_ms: 0,
            }],
        };
        assert_eq!(desired_web_port(&web), None);
        assert_eq!(desired_peer_port(&web), Some(27375));
    }

    #[test]
    fn discovery_targets_cover_multicast_global_and_subnet_broadcast() {
        let targets = discovery_targets(Some(Ipv4Addr::new(192, 168, 31, 100)));
        assert!(targets.contains(&SocketAddrV4::new(DISCOVERY_GROUP, DISCOVERY_PORT)));
        assert!(targets.contains(&SocketAddrV4::new(Ipv4Addr::BROADCAST, DISCOVERY_PORT)));
        assert!(targets.contains(&SocketAddrV4::new(
            Ipv4Addr::new(192, 168, 31, 255),
            DISCOVERY_PORT
        )));
    }
}

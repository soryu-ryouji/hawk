//! B 侧会话编排：发起会话 → 轮询 ready → mux 洪泛建连（指纹钉扎）→ 拉起会话代理。
//! 单活动会话模型（查看方同时只看一个远端库）：新会话自动顶替并断开旧会话。
//! 连接韧性（P4）：UDP 静默降级中继路径（mux 路径迁移）；连接死亡原会话内原地重建，
//! 重建连续失败才判定断开。会话生命周期经 SSE `remote.session` 事件推送（app 断线回切依赖）。

use crate::core::events::EventBus;
use crate::remote::data::{serve_proxy, ProxyContext};
use crate::remote::identity::Identity;
use crate::remote::mux::MuxSocket;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 会话建立轮询：1s 间隔、上限 30s（协议兜底线）
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const POLL_TIMEOUT: Duration = Duration::from_secs(30);

/// SSE 事件名（remote.session 载荷即 SessionEventPayload）
pub const REMOTE_SESSION_EVENT: &str = "remote.session";

/// 连接失败分类：Server = 服务端不可达/被拒（502）；Session = 会话状态语义（409）
#[derive(Debug)]
pub struct ConnectError {
    pub is_server: bool,
    pub message: String,
}

impl ConnectError {
    fn server(message: impl Into<String>) -> ConnectError {
        ConnectError {
            is_server: true,
            message: message.into(),
        }
    }

    fn session(message: impl Into<String>) -> ConnectError {
        ConnectError {
            is_server: false,
            message: message.into(),
        }
    }
}

type ConnectResult<T> = Result<T, ConnectError>;

#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct ActiveSessionDto {
    pub session_id: String,
    pub target_device_id: String,
    pub target_name: String,
    pub target_library_name: String,
    /// 会话代理端口（环回）：app 连接描述符的 api_base = http://127.0.0.1:<port>
    pub proxy_port: u16,
    /// 会话代理 token：web 查看器以 ?token= 携带
    pub token: String,
}

/// 会话传输层（endpoint + mux）：重建时整体替换，旧件随 drop 释放端口
struct SessionTransport {
    endpoint: quinn::Endpoint,
    mux: Arc<MuxSocket>,
}

struct ActiveSession {
    dto: ActiveSessionDto,
    /// 当前连接（与会话代理共享的单元格；P4 连接级重建热替换）
    conn_cell: Arc<std::sync::RwLock<quinn::Connection>>,
    transport: Mutex<SessionTransport>,
    proxy_shutdown: tokio::sync::oneshot::Sender<()>,
}

pub struct SessionManager {
    active: Mutex<Option<ActiveSession>>,
    /// 打洞预绑 socket（P3，session.probe 时创建并向服务端探测口报到，ready 后 connect
    /// 复用——探测与连接同端口是 observed endpoint 有效的前提）；残留条目随下次准备/断开清理
    pending_punch: Mutex<HashMap<String, std::net::UdpSocket>>,
    /// 应用级远程配置（中继回退开关：静默降级是否自动附加中继路径）
    remote: Arc<crate::remote::config::RemoteConfigStore>,
    bus: EventBus,
}

impl SessionManager {
    pub fn new(
        bus: EventBus,
        remote: Arc<crate::remote::config::RemoteConfigStore>,
    ) -> Arc<SessionManager> {
        Arc::new(SessionManager {
            active: Mutex::new(None),
            pending_punch: Mutex::new(HashMap::new()),
            remote,
            bus,
        })
    }

    /// B 侧打洞准备（WSS session.probe 触发，协议 P3）：预绑 ephemeral socket 并向
    /// 服务端探测口报到，socket 留存至 connect 复用。准备失败仅降级（服务端探测超时
    /// → unreachable），不影响后续会话
    pub fn prepare_punch(&self, session_id: &str, probe_addr: SocketAddr, token: [u8; 16]) {
        let socket = match bind_ephemeral_udp() {
            Ok(socket) => socket,
            Err(e) => {
                tracing::warn!("打洞预绑 socket 失败: {e}");
                return;
            }
        };
        if let Ok(clone) = socket.try_clone() {
            tokio::spawn(async move {
                crate::remote::data::send_probe_packets(&clone, probe_addr, token).await;
            });
        }
        let mut pending = self.pending_punch.lock().unwrap();
        // 单活动会话模型：其余会话的残留条目随本次准备一并清理
        pending.retain(|id, _| id == session_id);
        pending.insert(session_id.to_string(), socket);
    }

    pub fn snapshot(&self) -> Option<ActiveSessionDto> {
        self.active.lock().unwrap().as_ref().map(|s| s.dto.clone())
    }

    /// 测试挂钩：当前会话的 mux（迁移测试屏蔽 UDP 路径用）
    #[doc(hidden)]
    pub fn debug_active_mux(&self) -> Option<Arc<MuxSocket>> {
        self.active
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.transport.lock().unwrap().mux.clone())
    }

    /// 测试挂钩：当前连接的 stable_id（区分「迁移」与「重建」的硬证据）
    #[doc(hidden)]
    pub fn debug_active_stable_id(&self) -> Option<usize> {
        self.active
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.conn_cell.read().unwrap().stable_id())
    }

    /// 测试挂钩：掐断当前连接（模拟连接级死亡，驱动重建路径）
    #[doc(hidden)]
    pub fn debug_close_current_conn(&self) {
        if let Some(s) = self.active.lock().unwrap().as_ref() {
            s.conn_cell
                .read()
                .unwrap()
                .close(42u32.into(), b"debug kill");
        }
    }

    /// 建立到目标设备的远程会话：成功返回代理连接信息（app 据此切换连接描述符）。
    /// mode = direct（直连候选）/ relay（服务端中继，见协议「中继」一节）
    pub async fn connect(
        self: &Arc<SessionManager>,
        http: &reqwest::Client,
        base: &str,
        key: &str,
        identity: &Identity,
        target_device_id: &str,
        mode: &str,
    ) -> ConnectResult<ActiveSessionDto> {
        let self_device_id = identity
            .device_id
            .clone()
            .ok_or_else(|| ConnectError::session("设备未注册（先启用远程接入并完成注册）"))?;

        // ---- 发起会话 ----
        let session_id =
            create_session(http, base, key, &self_device_id, target_device_id, mode).await?;

        // ---- 轮询 ready（WSS 推送为增强，轮询为协议兜底线） ----
        let connect_info = poll_ready(http, base, key, &session_id).await?;
        let expected = parse_connect_fingerprint(&connect_info)?;

        // ---- 建立数据面（P4 mux 统一 endpoint）：relay 附加中继路径；direct 候选洪泛 ----
        let (endpoint, mux, conn) = if mode == "relay" {
            let ws = crate::remote::relay::connect_relay_ws(
                base,
                key,
                &self_device_id,
                &session_id,
                "b",
            )
            .await
            .map_err(ConnectError::server)?;
            let socket = bind_ephemeral_udp().map_err(ConnectError::session)?;
            let mux = MuxSocket::new_client(socket, Vec::new());
            mux.add_relay_path(ws);
            let (endpoint, conn) = crate::remote::data::connect_mux(
                identity,
                mux.clone(),
                expected,
                RELAY_CONNECT_TIMEOUT,
            )
            .await
            .map_err(ConnectError::session)?;
            (endpoint, mux, conn)
        } else {
            let candidates = connect_info["candidates"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            // 候选洪泛（P4，ICE/Tailscale 同款的 mux 版）：握手 Initial 发往全部候选地址，
            // 首个入站数据报锁定路径。串行最坏 25s+ → 洪泛典型 ~1s
            let punch_present = candidates
                .iter()
                .any(|c| c["transport"].as_str() == Some("punch"));
            let flood: Vec<SocketAddr> = candidates
                .iter()
                .filter_map(|c| {
                    Some(SocketAddr::new(
                        c["host"].as_str()?.parse().ok()?,
                        c["port"].as_u64()? as u16,
                    ))
                })
                .collect();
            // P3 打洞：复用 session.probe 时预绑的 socket（同端口前提）；缺失时退普通
            // socket（打洞失效，其余候选不受影响）
            let socket = if punch_present {
                match self.pending_punch.lock().unwrap().remove(&session_id) {
                    Some(socket) => socket,
                    None => {
                        tracing::debug!("打洞预绑 socket 缺失（session.probe 未收到），打洞失效");
                        bind_ephemeral_udp().map_err(ConnectError::session)?
                    }
                }
            } else {
                bind_ephemeral_udp().map_err(ConnectError::session)?
            };
            let timeout = if punch_present {
                crate::remote::data::PUNCH_TIMEOUT
            } else {
                crate::remote::data::CANDIDATE_TIMEOUT
            };
            let mux = MuxSocket::new_client(socket, flood);
            match crate::remote::data::connect_mux(identity, mux.clone(), expected, timeout).await {
                Ok((endpoint, conn)) => (endpoint, mux, conn),
                Err(e) => {
                    let _ = cancel_session(http, base, key, &session_id).await;
                    return Err(ConnectError::session(format!("直连失败：{e}")));
                }
            }
        };

        // ---- 目标信息（label 展示用，尽力而为） ----
        let (target_name, target_library_name) =
            fetch_target_label(http, base, key, target_device_id).await;

        // ---- 拉起会话代理 ----
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| ConnectError::server(format!("会话代理绑定失败: {e}")))?;
        let proxy_port = listener
            .local_addr()
            .map_err(|e| ConnectError::server(format!("会话代理地址获取失败: {e}")))?
            .port();
        let token = random_token();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        // 连接单元格：代理与监控任务共享，重建时热替换（P4）
        let conn_cell = Arc::new(std::sync::RwLock::new(conn));
        let ctx = Arc::new(ProxyContext {
            conn: conn_cell.clone(),
            token: token.clone(),
            session_id: session_id.clone(),
        });
        tokio::spawn(async move {
            tokio::select! {
                _ = serve_proxy(ctx, listener) => {}
                _ = shutdown_rx => {}
            }
        });

        let dto = ActiveSessionDto {
            session_id: session_id.clone(),
            target_device_id: target_device_id.to_string(),
            target_name,
            target_library_name,
            proxy_port,
            token,
        };

        // ---- 顶替旧会话（断开并尽力取消）。锁 guard 不跨 await：先取出再处理 ----
        let old = self.active.lock().unwrap().replace(ActiveSession {
            dto: dto.clone(),
            conn_cell: conn_cell.clone(),
            transport: Mutex::new(SessionTransport {
                endpoint,
                mux: mux.clone(),
            }),
            proxy_shutdown: shutdown_tx,
        });
        if let Some(old) = old {
            let old_session_id = old.dto.session_id.clone();
            old.teardown();
            let _ = cancel_session(http, base, key, &old_session_id).await;
        }

        // ---- 断线监控（P4：先原地重建，重建连续失败才判定断开） ----
        {
            let manager = self.clone();
            let http = http.clone();
            let base = base.to_string();
            let key = key.to_string();
            let identity = identity.clone();
            let session_id = dto.session_id.clone();
            let target_device_id = dto.target_device_id.clone();
            let mode = mode.to_string();
            tokio::spawn(async move {
                watch_session(
                    manager,
                    http,
                    base,
                    key,
                    identity,
                    session_id,
                    target_device_id,
                    mode,
                    conn_cell,
                )
                .await;
            });
        }

        // ---- UDP 静默降级观察（P4，仅 direct 会话；中继路径由 WSS 推送驱动附加） ----
        if mode == "direct" {
            let manager = self.clone();
            let http = http.clone();
            let base = base.to_string();
            let key = key.to_string();
            let session_id = dto.session_id.clone();
            let target_device_id = dto.target_device_id.clone();
            tokio::spawn(async move {
                degrade_watch(manager, http, base, key, session_id, target_device_id, mux).await;
            });
        }

        self.publish(&dto.session_id, &dto.target_device_id, "connected");
        Ok(dto)
    }

    /// 中继路径并入当前会话 mux（WSS session.relay_path 驱动）；会话已更替返回 false
    pub fn attach_relay_to_active(
        &self,
        session_id: &str,
        ws: crate::remote::relay::RelayWs,
    ) -> bool {
        let active = self.active.lock().unwrap();
        match active.as_ref() {
            Some(s) if s.dto.session_id == session_id => {
                s.transport.lock().unwrap().mux.add_relay_path(ws);
                true
            }
            _ => false,
        }
    }

    /// 当前活动会话是否仍是指定会话（监控/观察任务与顶替、断开的竞态裁决）
    fn is_current(&self, session_id: &str) -> bool {
        self.active
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s.dto.session_id == session_id)
    }

    /// 断开当前会话（幂等）：通知服务端取消 + 本地收尾
    pub async fn disconnect(&self, http: &reqwest::Client, base: &str, key: &str) {
        self.pending_punch.lock().unwrap().clear();
        let old = self.active.lock().unwrap().take();
        if let Some(old) = old {
            let session_id = old.dto.session_id.clone();
            let target_device_id = old.dto.target_device_id.clone();
            old.teardown();
            let _ = cancel_session(http, base, key, &session_id).await;
            self.publish(&session_id, &target_device_id, "disconnected");
        }
    }

    /// 仅当当前会话仍是指定会话时清除（断线监控与主动断开的竞态裁决）；返回是否发生了清除
    fn clear_if_current(&self, session_id: &str) -> bool {
        let mut active = self.active.lock().unwrap();
        if active
            .as_ref()
            .is_some_and(|s| s.dto.session_id == session_id)
        {
            if let Some(old) = active.take() {
                old.teardown_silent();
            }
            return true;
        }
        false
    }

    fn publish(&self, session_id: &str, target_device_id: &str, state: &str) {
        self.bus.publish(
            REMOTE_SESSION_EVENT,
            json!({
                "session_id": session_id,
                "target_device_id": target_device_id,
                "state": state,
            }),
        );
    }
}

impl ActiveSession {
    /// 主动收尾：断隧道 + 停代理 + 释放传输层
    fn teardown(self) {
        let _ = self.proxy_shutdown.send(());
        self.conn_cell
            .read()
            .unwrap()
            .close(0u32.into(), b"session closed");
        let transport = self.transport.lock().unwrap();
        transport.mux.close();
        transport.endpoint.close(0u32.into(), b"session closed");
    }

    /// 断线监控收尾：连接已死，只停代理与释放传输层
    fn teardown_silent(self) {
        let _ = self.proxy_shutdown.send(());
        let transport = self.transport.lock().unwrap();
        transport.mux.close();
        transport.endpoint.close(0u32.into(), b"session ended");
    }
}

/// 中继会话握手超时：15s。中继比直连多一串串行链路（accept RTT → 对端经反代
/// 连 relay WS → 配对 → WS 上的 QUIC 往返），直连的 5s 在跨网中继下过紧
const RELAY_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// 断线监控（P4）：连接死亡先在原会话内原地重建（路径迁移救不回的兜底层）；
/// 连续 3 次重建失败才判定断开（SSE disconnected，app 回切本地库）。
/// relay 模式会话唯一路径即中继，连接死亡即终态（重建语义交给新一轮中继确认流程）
#[allow(clippy::too_many_arguments)]
async fn watch_session(
    manager: Arc<SessionManager>,
    http: reqwest::Client,
    base: String,
    key: String,
    identity: Identity,
    session_id: String,
    target_device_id: String,
    mode: String,
    conn_cell: Arc<std::sync::RwLock<quinn::Connection>>,
) {
    let mut failures = 0u32;
    loop {
        let conn = conn_cell.read().unwrap().clone();
        conn.closed().await;
        if !manager.is_current(&session_id) {
            return; // 已被顶替或主动断开：收尾由对应路径负责
        }
        if mode != "direct" {
            break;
        }
        failures += 1;
        if failures > 3 {
            break;
        }
        let backoff = Duration::from_secs(1 << (failures - 1)); // 1s/2s/4s
        tracing::info!(
            "远程会话 {session_id} 连接断开，{backoff:?} 后原地重建（第 {failures} 次）"
        );
        tokio::time::sleep(backoff).await;
        match rebuild_once(&http, &base, &key, &identity, &session_id).await {
            Ok((endpoint, mux, conn)) => {
                // 热替换（重建期间会话可能已被顶替/断开，锁内复核）
                let active = manager.active.lock().unwrap();
                let Some(current) = active.as_ref() else {
                    conn.close(0u32.into(), b"superseded");
                    return;
                };
                if current.dto.session_id != session_id {
                    conn.close(0u32.into(), b"superseded");
                    return;
                }
                *conn_cell.write().unwrap() = conn;
                *current.transport.lock().unwrap() = SessionTransport { endpoint, mux };
                drop(active);
                failures = 0;
                tracing::info!("远程会话 {session_id} 已原地重建（连接级路径迁移）");
            }
            Err(e) => tracing::debug!("会话 {session_id} 重建失败: {e}"),
        }
    }
    if manager.clear_if_current(&session_id) {
        tracing::info!("远程会话 {session_id} 隧道已断开（重建失败）");
        manager.publish(&session_id, &target_device_id, "disconnected");
    }
}

/// 原会话内重建连接：重取连接信息（服务端会话仍 ready）→ 排除打洞候选（observed
/// 端点在重建时可能已过期）→ 新 socket + mux 洪泛其余候选
async fn rebuild_once(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    identity: &Identity,
    session_id: &str,
) -> Result<(quinn::Endpoint, Arc<MuxSocket>, quinn::Connection), String> {
    let resp = http
        .get(format!("{base}/api/v1/sessions/{session_id}"))
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| format!("会话状态查询失败: {e}"))?;
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("会话状态解析失败: {e}"))?;
    if body["data"]["state"].as_str() != Some("ready") {
        return Err(format!("会话已非 ready（{}）", body["data"]["state"]));
    }
    let connect = body["data"]["connect"].clone();
    let expected = parse_connect_fingerprint(&connect).map_err(|e| e.message)?;
    let flood: Vec<SocketAddr> = connect["candidates"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|c| c["transport"].as_str() != Some("punch"))
        .filter_map(|c| {
            Some(SocketAddr::new(
                c["host"].as_str()?.parse().ok()?,
                c["port"].as_u64()? as u16,
            ))
        })
        .collect();
    let socket = bind_ephemeral_udp()?;
    let mux = MuxSocket::new_client(socket, flood);
    crate::remote::data::connect_mux(
        identity,
        mux.clone(),
        expected,
        crate::remote::data::CANDIDATE_TIMEOUT,
    )
    .await
    .map(|(endpoint, conn)| (endpoint, mux, conn))
}

/// UDP 静默降级观察（P4）：mux 观测的 UDP 入站静默超阈值时，中继回退已开启则
/// 请求服务端附加中继路径（WSS session.relay_path 驱动双方接入），否则只发布
/// degraded 事件（连接将随 idle 超时断开，走重建/断开路径）
async fn degrade_watch(
    manager: Arc<SessionManager>,
    http: reqwest::Client,
    base: String,
    key: String,
    session_id: String,
    target_device_id: String,
    mux: Arc<MuxSocket>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        interval.tick().await;
        if !manager.is_current(&session_id) {
            return;
        }
        if mux.relay_attached() {
            return; // 已在中继上；UDP 恢复时 mux 出站路由自动回切（路径活性驱动）
        }
        if mux.udp_silence() > crate::remote::mux::silence_threshold() {
            if manager.remote.current().relay_fallback {
                tracing::warn!(
                    "会话 {session_id} UDP 路径静默超阈值，附加中继路径（中继回退已开启）"
                );
                let resp = http
                    .post(format!("{base}/api/v1/sessions/{session_id}/relay-path"))
                    .bearer_auth(&key)
                    .send()
                    .await;
                match resp {
                    Ok(resp) if resp.status().is_success() => {}
                    Ok(resp) => tracing::warn!("中继路径附加被拒（{}）", resp.status()),
                    Err(e) => tracing::warn!("中继路径附加失败: {e}"),
                }
            } else {
                tracing::warn!(
                    "会话 {session_id} UDP 路径静默超阈值（中继回退未开启，连接将随 idle 超时断开）"
                );
            }
            manager.publish(&session_id, &target_device_id, "degraded");
            return;
        }
    }
}

async fn create_session(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    self_device_id: &str,
    target_device_id: &str,
    mode: &str,
) -> ConnectResult<String> {
    let resp = http
        .post(format!("{base}/api/v1/sessions"))
        .bearer_auth(key)
        .json(&json!({
            "source_device_id": self_device_id,
            "target_device_id": target_device_id,
            "mode": mode,
        }))
        .send()
        .await
        .map_err(|e| ConnectError::server(format!("会话发起失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(ConnectError::server(server_message(resp).await));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| ConnectError::server(format!("会话响应解析失败: {e}")))?;
    if body["data"]["state"] == "rejected" {
        return Err(ConnectError::session("对方设备已关闭远程访问"));
    }
    body["data"]["session_id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| ConnectError::server("会话响应缺少 session_id"))
}

/// 轮询会话状态直到 ready/终态；返回 connect 对象（candidates + fingerprint）
async fn poll_ready(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    session_id: &str,
) -> ConnectResult<Value> {
    let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
    loop {
        let resp = http
            .get(format!("{base}/api/v1/sessions/{session_id}"))
            .bearer_auth(key)
            .send()
            .await
            .map_err(|e| ConnectError::server(format!("会话状态查询失败: {e}")))?;
        if !resp.status().is_success() {
            return Err(ConnectError::server(server_message(resp).await));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ConnectError::server(format!("会话状态解析失败: {e}")))?;
        match body["data"]["state"].as_str().unwrap_or_default() {
            "ready" => return Ok(body["data"]["connect"].clone()),
            "rejected" => return Err(ConnectError::session("对方拒绝")),
            "unreachable" => {
                return Err(ConnectError::session(
                    "目标网络不可达：无直连候选（无 IPv4 映射且无全局 IPv6）",
                ))
            }
            "expired" => return Err(ConnectError::session("会话建立超时")),
            "cancelled" => return Err(ConnectError::session("会话已取消")),
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ConnectError::session("会话建立超时"));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn parse_connect_fingerprint(connect_info: &Value) -> ConnectResult<[u8; 32]> {
    let hex_str = connect_info["fingerprint"]
        .as_str()
        .ok_or_else(|| ConnectError::server("会话连接信息缺少指纹"))?;
    crate::remote::tls::parse_fingerprint(hex_str)
        .ok_or_else(|| ConnectError::server("会话连接信息指纹非法"))
}

async fn fetch_target_label(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    target_device_id: &str,
) -> (String, String) {
    let fallback = ("远端设备".to_string(), "素材库".to_string());
    let Ok(resp) = http
        .get(format!("{base}/api/v1/devices"))
        .bearer_auth(key)
        .send()
        .await
    else {
        return fallback;
    };
    let Ok(body) = resp.json::<Value>().await else {
        return fallback;
    };
    let Some(device) = body["data"]["devices"]
        .as_array()
        .and_then(|devices| devices.iter().find(|d| d["device_id"] == target_device_id))
    else {
        return fallback;
    };
    (
        device["name"].as_str().unwrap_or("远端设备").to_string(),
        device["library_name"]
            .as_str()
            .unwrap_or("素材库")
            .to_string(),
    )
}

async fn cancel_session(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    session_id: &str,
) -> Result<(), String> {
    http.post(format!("{base}/api/v1/sessions/{session_id}/cancel"))
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| format!("会话取消失败: {e}"))?;
    Ok(())
}

/// 服务端错误信封的 message（协议：message 直接展示）
async fn server_message(resp: reqwest::Response) -> String {
    let status = resp.status();
    resp.json::<Value>()
        .await
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| format!("服务端返回 {status}"))
}

/// B 侧打洞预绑：v4 原生 ephemeral socket，已设非阻塞（connect 复用时交 quinn）。
/// 协议 P3 的探测/打洞目标恒为 v4（服务端探测口仅绑 IPv4，observed endpoint 必为 v4）；
/// v4 原生同时绕开 Windows 双栈 socket 的语义差异（v6only 默认值的平台分歧）
fn bind_ephemeral_udp() -> Result<std::net::UdpSocket, String> {
    let socket = std::net::UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0)))
        .map_err(|e| format!("UDP socket 绑定失败: {e}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|e| format!("UDP socket 非阻塞设置失败: {e}"))?;
    Ok(socket)
}

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("系统随机源可用");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

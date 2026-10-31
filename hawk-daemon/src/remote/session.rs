//! B 侧会话编排：发起会话 → 轮询 ready → iroh 拨号（EndpointAddr：endpoint_id + 直连地址
//! + 可选中继）→ 拉起会话代理。传输层的打洞/中继竞速/路径迁移/保活全部是 iroh 内部事务。
//!
//! 连接韧性：连接死亡在原会话内重拨并热替换进代理（前端无感），连续失败才判定断开。
//! 单活动会话模型（查看方同时只看一个远端库）：新会话自动顶替并断开旧会话。
//! 会话生命周期经 SSE `remote.session` 事件推送（app 断线回切依赖）。

use crate::core::access::Role;
use crate::core::events::EventBus;
use crate::remote::data::{serve_proxy, ProxyContext, CONNECT_TIMEOUT, HAWK_ALPN};
use crate::remote::identity::Identity;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// 会话建立轮询：1s 间隔、上限 30s（协议兜底线）
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const POLL_TIMEOUT: Duration = Duration::from_secs(30);

/// SSE 事件名（remote.session 载荷即 SessionEventPayload）
pub const REMOTE_SESSION_EVENT: &str = "remote.session";

/// 连接失败分类：Server = 服务端不可达/被拒（502）；Session = 会话状态语义（409）；
/// write_denied = 可写会话被拒（目标未开启广域网写入），调用方降级只读重试
#[derive(Debug)]
pub struct ConnectError {
    pub is_server: bool,
    pub write_denied: bool,
    pub message: String,
}

impl ConnectError {
    fn server(message: impl Into<String>) -> ConnectError {
        ConnectError {
            is_server: true,
            write_denied: false,
            message: message.into(),
        }
    }

    fn session(message: impl Into<String>) -> ConnectError {
        ConnectError {
            is_server: false,
            write_denied: false,
            message: message.into(),
        }
    }

    fn write_denied(message: impl Into<String>) -> ConnectError {
        ConnectError {
            is_server: false,
            write_denied: true,
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
    /// 会话代理临时凭证：仅以 Authorization Bearer 使用
    pub session_token: String,
    /// 会话角色：viewer 只读，importer 允许导入（目标设备仍逐请求执法）
    pub role: Role,
}

struct ActiveSession {
    dto: ActiveSessionDto,
    access_rendezvous: bool,
    /// Device Session 或 rendezvous 票据；用于重拨和取消，避免不同会话类型串用凭证。
    control_key: String,
    /// 当前连接（与会话代理共享的单元格；断线重拨热替换）
    conn_cell: Arc<RwLock<iroh::endpoint::Connection>>,
    proxy_shutdown: tokio::sync::oneshot::Sender<()>,
}

/// 数据面句柄槽：signal 任务持有 DataPlane 生命周期，拨号方经此槽取 endpoint
///（None = 远程接入中/未启用，会话此刻不可建立）
pub type EndpointSlot = Arc<RwLock<Option<Arc<crate::remote::data::DataPlane>>>>;

pub struct SessionManager {
    active: Mutex<Option<ActiveSession>>,
    /// 应用级远程配置（设备级中继开关：不使用中继时拨号不带 relay_url）
    remote: Arc<crate::remote::config::RemoteConfigStore>,
    endpoint_slot: EndpointSlot,
    /// AccessKey-only 查看方的按需拨号端点；不占用固定 QUIC 端口，也不要求启用本机 remote。
    access_plane: tokio::sync::Mutex<Option<Arc<crate::remote::data::DataPlane>>>,
    bus: EventBus,
}

impl SessionManager {
    pub fn new(
        bus: EventBus,
        remote: Arc<crate::remote::config::RemoteConfigStore>,
        endpoint_slot: EndpointSlot,
    ) -> Arc<SessionManager> {
        Arc::new(SessionManager {
            active: Mutex::new(None),
            remote,
            endpoint_slot,
            access_plane: tokio::sync::Mutex::new(None),
            bus,
        })
    }

    pub fn snapshot(&self) -> Option<ActiveSessionDto> {
        self.active.lock().unwrap().as_ref().map(|s| s.dto.clone())
    }

    /// 测试挂钩：当前连接的 stable_id（区分「迁移」与「重拨」的硬证据）
    #[doc(hidden)]
    pub fn debug_active_stable_id(&self) -> Option<usize> {
        self.active
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.conn_cell.read().unwrap().stable_id())
    }

    /// 测试挂钩：掐断当前连接（模拟连接级死亡，驱动重拨路径）
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
    /// 可写优先：目标开启「允许广域网保存素材」时争取写会话，被拒则降级只读
    /// （浏览本身不该因为不能写而失败）。
    pub async fn connect(
        self: &Arc<SessionManager>,
        http: &reqwest::Client,
        base: &str,
        key: &str,
        identity: &Identity,
        target_device_id: &str,
    ) -> ConnectResult<ActiveSessionDto> {
        match self
            .connect_once(http, base, key, identity, target_device_id, true)
            .await
        {
            Err(e) if e.write_denied => {
                tracing::info!("可写会话被拒（{}），降级只读重连", e.message);
                self.connect_once(http, base, key, identity, target_device_id, false)
                    .await
            }
            other => other,
        }
    }

    async fn connect_once(
        self: &Arc<SessionManager>,
        http: &reqwest::Client,
        base: &str,
        key: &str,
        identity: &Identity,
        target_device_id: &str,
        writable: bool,
    ) -> ConnectResult<ActiveSessionDto> {
        // ---- 发起会话（source 由 Device Session 决定） ----
        let session_id = create_session(http, base, key, target_device_id, writable).await?;

        // ---- 轮询 ready（WSS 推送为增强，轮询为协议兜底线） ----
        let connect_info = poll_ready(http, base, key, &session_id, writable).await?;

        // ---- 建立数据面：组装 EndpointAddr 并拨号 ----
        let conn = match self.dial(identity, &connect_info).await {
            Ok(conn) => conn,
            Err(e) => {
                let _ = cancel_session(http, base, key, &session_id).await;
                return Err(ConnectError::session(e));
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
        let session_token = random_session_token();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let conn_cell = Arc::new(RwLock::new(conn));
        let ctx = Arc::new(ProxyContext {
            conn: conn_cell.clone(),
            session_token: session_token.clone(),
            session_id: session_id.clone(),
            role: std::sync::RwLock::new(if writable {
                Role::Importer
            } else {
                Role::Viewer
            }),
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
            session_token,
            role: if writable {
                Role::Importer
            } else {
                Role::Viewer
            },
        };

        // ---- 顶替旧会话（断开并尽力取消）。锁 guard 不跨 await：先取出再处理 ----
        let old = self.active.lock().unwrap().replace(ActiveSession {
            dto: dto.clone(),
            access_rendezvous: false,
            control_key: key.to_string(),
            conn_cell: conn_cell.clone(),
            proxy_shutdown: shutdown_tx,
        });
        if let Some(old) = old {
            let old_session_id = old.dto.session_id.clone();
            let access_rendezvous = old.access_rendezvous;
            let old_key = old.control_key.clone();
            old.teardown();
            let _ = if access_rendezvous {
                cancel_rendezvous(http, base, &old_key, &old_session_id).await
            } else {
                cancel_session(http, base, &old_key, &old_session_id).await
            };
        }

        // ---- 断线监控：连接死亡 → 原会话内重拨（热替换）；连续失败才判定断开 ----
        {
            let manager = self.clone();
            let http = http.clone();
            let base = base.to_string();
            let key = key.to_string();
            let identity = identity.clone();
            let session_id = dto.session_id.clone();
            let target_device_id = dto.target_device_id.clone();
            tokio::spawn(async move {
                watch_session(
                    manager,
                    http,
                    base,
                    key,
                    identity,
                    session_id,
                    target_device_id,
                    conn_cell,
                    false,
                )
                .await;
            });
        }

        spawn_lease_watch(
            self.clone(),
            http.clone(),
            base.to_string(),
            key.to_string(),
            dto.session_id.clone(),
            dto.target_device_id.clone(),
            dto.proxy_port,
            dto.session_token.clone(),
            false,
        );
        self.publish(&dto.session_id, &dto.target_device_id, "connected");
        Ok(dto)
    }

    /// AccessKey-only 原生连接：匿名 rendezvous + target_identity 钉扎 + 隧道内登录。
    pub async fn connect_access(
        self: &Arc<SessionManager>,
        http: &reqwest::Client,
        base: &str,
        identity: &Identity,
        target_device_id: &str,
        target_identity: &str,
        access_key: &str,
    ) -> ConnectResult<ActiveSessionDto> {
        let (session_id, rendezvous_token) =
            create_rendezvous(http, base, identity, target_device_id).await?;
        let connect_info =
            poll_rendezvous_ready(http, base, &rendezvous_token, &session_id).await?;
        let actual_identity = connect_info["endpoint_id"].as_str().unwrap_or_default();
        if actual_identity != target_identity {
            let _ = cancel_rendezvous(http, base, &rendezvous_token, &session_id).await;
            return Err(ConnectError::session(
                "目标身份与配对信息不一致，已拒绝连接",
            ));
        }
        let conn = match self.dial(identity, &connect_info).await {
            Ok(conn) => conn,
            Err(error) => {
                let _ = cancel_rendezvous(http, base, &rendezvous_token, &session_id).await;
                return Err(ConnectError::session(error));
            }
        };

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| ConnectError::server(format!("会话代理绑定失败: {e}")))?;
        let proxy_port = listener
            .local_addr()
            .map_err(|e| ConnectError::server(format!("会话代理地址获取失败: {e}")))?
            .port();
        let session_token = random_session_token();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let conn_cell = Arc::new(RwLock::new(conn));
        let ctx = Arc::new(ProxyContext {
            conn: conn_cell.clone(),
            session_token: session_token.clone(),
            session_id: session_id.clone(),
            role: std::sync::RwLock::new(Role::Viewer),
        });
        let proxy_ctx = ctx.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = serve_proxy(proxy_ctx, listener) => {}
                _ = shutdown_rx => {}
            }
        });

        let role = match login_access_key(http, proxy_port, &session_token, access_key).await {
            Ok(role) => role,
            Err(error) => {
                let _ = shutdown_tx.send(());
                let _ = cancel_rendezvous(http, base, &rendezvous_token, &session_id).await;
                return Err(error);
            }
        };
        *ctx.role.write().unwrap() = role;
        let dto = ActiveSessionDto {
            session_id: session_id.clone(),
            target_device_id: target_device_id.to_string(),
            target_name: target_device_id.to_string(),
            target_library_name: String::new(),
            proxy_port,
            session_token,
            role,
        };

        let old = self.active.lock().unwrap().replace(ActiveSession {
            dto: dto.clone(),
            access_rendezvous: true,
            control_key: rendezvous_token.clone(),
            conn_cell: conn_cell.clone(),
            proxy_shutdown: shutdown_tx,
        });
        if let Some(old) = old {
            let old_session_id = old.dto.session_id.clone();
            let access_rendezvous = old.access_rendezvous;
            let old_key = old.control_key.clone();
            old.teardown();
            let _ = if access_rendezvous {
                cancel_rendezvous(http, base, &old_key, &old_session_id).await
            } else {
                cancel_session(http, base, &old_key, &old_session_id).await
            };
        }

        {
            let manager = self.clone();
            let http = http.clone();
            let base = base.to_string();
            let identity = identity.clone();
            let session_id = dto.session_id.clone();
            let target_device_id = dto.target_device_id.clone();
            let watch_key = rendezvous_token.clone();
            tokio::spawn(async move {
                watch_session(
                    manager,
                    http,
                    base,
                    watch_key,
                    identity,
                    session_id,
                    target_device_id,
                    conn_cell,
                    true,
                )
                .await;
            });
        }
        spawn_lease_watch(
            self.clone(),
            http.clone(),
            base.to_string(),
            rendezvous_token.clone(),
            dto.session_id.clone(),
            dto.target_device_id.clone(),
            dto.proxy_port,
            dto.session_token.clone(),
            true,
        );
        self.publish(&dto.session_id, &dto.target_device_id, "connected");
        Ok(dto)
    }

    /// 拨号：connect 对象 → EndpointAddr → iroh connect（共享常驻 endpoint）。
    /// 设备级中继开关关闭时不携带 relay_url（iroh 就只走直连路径）
    async fn dial(
        &self,
        identity: &Identity,
        connect_info: &Value,
    ) -> Result<iroh::endpoint::Connection, String> {
        let endpoint_id: iroh::EndpointId = connect_info["endpoint_id"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| "会话连接信息缺少 endpoint_id".to_string())?;
        let mut addr = iroh::EndpointAddr::new(endpoint_id);
        if let Some(addrs) = connect_info["direct_addresses"].as_array() {
            for a in addrs {
                if let Some(sock) = a.as_str().and_then(|s| s.parse().ok()) {
                    addr = addr.with_ip_addr(sock);
                }
            }
        }
        let relay_on = self.remote.current().relay;
        if relay_on {
            if let Some(url) = connect_info["relay"]["url"].as_str() {
                if let Ok(relay_url) = url.parse() {
                    addr = addr.with_relay_url(relay_url);
                }
            }
        }
        let existing = { self.endpoint_slot.read().unwrap().clone() };
        let plane = if let Some(plane) = existing {
            plane
        } else {
            let mut owned = self.access_plane.lock().await;
            if owned.is_none() {
                *owned = Some(
                    crate::remote::data::DataPlane::bind(identity, 0, None, axum::Router::new())
                        .await
                        .map_err(|e| format!("AccessKey 拨号端点初始化失败: {e}"))?,
                );
            }
            owned.as_ref().unwrap().clone()
        };
        let Some(endpoint) = plane.endpoint() else {
            return Err("数据面已关闭".to_string());
        };
        let _ = identity; // endpoint 身份即本机身份（拨号不再需要另行出示）
        tokio::time::timeout(CONNECT_TIMEOUT, endpoint.connect(addr, HAWK_ALPN))
            .await
            .map_err(|_| {
                // 附传输线索：中继是被本机开关丢弃的 / 服务端没提供中继 / 直连地址不可达
                let relay_note = if connect_info["relay"]["url"].is_string() && !relay_on {
                    "（服务端提供中继但本机未开启「使用中继」）"
                } else if connect_info["relay"]["url"].is_string() {
                    "（中继在场仍超时——检查 relay 可达性与对端在线）"
                } else {
                    "（该部署未提供中继，纯直连形态）"
                };
                format!("隧道握手超时（{CONNECT_TIMEOUT:?}）{relay_note}")
            })?
            .map_err(|e| format!("隧道握手失败: {e}"))
    }

    /// 断开当前会话（幂等）：通知服务端取消 + 本地收尾
    pub async fn disconnect(&self, http: &reqwest::Client, base: &str, _key: &str) {
        let old = self.active.lock().unwrap().take();
        if let Some(old) = old {
            let session_id = old.dto.session_id.clone();
            let target_device_id = old.dto.target_device_id.clone();
            let access_rendezvous = old.access_rendezvous;
            let old_key = old.control_key.clone();
            old.teardown();
            let _ = if access_rendezvous {
                cancel_rendezvous(http, base, &old_key, &session_id).await
            } else {
                cancel_session(http, base, &old_key, &session_id).await
            };
            self.publish(&session_id, &target_device_id, "disconnected");
        }
    }

    /// 当前活动会话是否仍是指定会话（监控任务与顶替、断开的竞态裁决）
    fn is_current(&self, session_id: &str) -> bool {
        self.active
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s.dto.session_id == session_id)
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
    /// 主动收尾：断隧道 + 停代理
    fn teardown(self) {
        let _ = self.proxy_shutdown.send(());
        self.conn_cell
            .read()
            .unwrap()
            .close(0u32.into(), b"session closed");
    }

    /// 断线监控收尾：连接已死，只停代理
    fn teardown_silent(self) {
        let _ = self.proxy_shutdown.send(());
    }
}

const LEASE_RENEW_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[allow(clippy::too_many_arguments)]
fn spawn_lease_watch(
    manager: Arc<SessionManager>,
    http: reqwest::Client,
    base: String,
    key: String,
    session_id: String,
    target_device_id: String,
    proxy_port: u16,
    proxy_token: String,
    access_rendezvous: bool,
) {
    tokio::spawn(async move {
        let mut failures = 0u32;
        loop {
            tokio::time::sleep(LEASE_RENEW_INTERVAL).await;
            if !manager.is_current(&session_id) {
                return;
            }
            let path = if access_rendezvous {
                format!("/api/v1/rendezvous/{session_id}")
            } else {
                format!("/api/v1/sessions/{session_id}")
            };
            let server_ok = http
                .get(format!("{base}{path}"))
                .bearer_auth(&key)
                .send()
                .await
                .is_ok_and(|resp| resp.status().is_success());
            let lease_ok = if server_ok {
                http.post(format!("http://127.0.0.1:{proxy_port}/api/v1/remote/lease"))
                    .bearer_auth(&proxy_token)
                    .send()
                    .await
                    .is_ok_and(|resp| resp.status().is_success())
            } else {
                false
            };
            if lease_ok {
                failures = 0;
                continue;
            }
            failures += 1;
            if failures >= 3 && manager.clear_if_current(&session_id) {
                manager.publish(&session_id, &target_device_id, "disconnected");
                return;
            }
        }
    });
}

/// 断线监控：连接死亡先在原会话内重拨（iroh 路径迁移救不回的兜底层）；
/// 连续 3 次重拨失败才判定断开（SSE disconnected，app 回切本地库）
#[allow(clippy::too_many_arguments)]
async fn watch_session(
    manager: Arc<SessionManager>,
    http: reqwest::Client,
    base: String,
    key: String,
    identity: Identity,
    session_id: String,
    target_device_id: String,
    conn_cell: Arc<RwLock<iroh::endpoint::Connection>>,
    access_rendezvous: bool,
) {
    let mut failures = 0u32;
    loop {
        let conn = conn_cell.read().unwrap().clone();
        conn.closed().await;
        if !manager.is_current(&session_id) {
            return; // 已被顶替或主动断开：收尾由对应路径负责
        }
        failures += 1;
        if failures > 3 {
            break;
        }
        let backoff = Duration::from_secs(1 << (failures - 1)); // 1s/2s/4s
        tracing::info!("远程会话 {session_id} 连接断开，{backoff:?} 后重拨（第 {failures} 次）");
        tokio::time::sleep(backoff).await;
        match rebuild_once(
            &manager,
            &http,
            &base,
            &key,
            &identity,
            &session_id,
            access_rendezvous,
        )
        .await
        {
            Ok(conn) => {
                *conn_cell.write().unwrap() = conn;
                failures = 0;
                tracing::info!("远程会话 {session_id} 已原地重拨恢复");
            }
            Err(e) => tracing::debug!("会话 {session_id} 重拨失败: {e}"),
        }
    }
    if manager.clear_if_current(&session_id) {
        tracing::info!("远程会话 {session_id} 隧道已断开（重拨失败）");
        manager.publish(&session_id, &target_device_id, "disconnected");
    }
}

/// 原会话内重拨：重取连接信息（服务端会话仍 ready）→ 再走一遍拨号
async fn rebuild_once(
    manager: &Arc<SessionManager>,
    http: &reqwest::Client,
    base: &str,
    key: &str,
    identity: &Identity,
    session_id: &str,
    access_rendezvous: bool,
) -> Result<iroh::endpoint::Connection, String> {
    let path = if access_rendezvous {
        format!("/api/v1/rendezvous/{session_id}")
    } else {
        format!("/api/v1/sessions/{session_id}")
    };
    let resp = http
        .get(format!("{base}{path}"))
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
    manager.dial(identity, &body["data"]["connect"]).await
}

async fn create_rendezvous(
    http: &reqwest::Client,
    base: &str,
    identity: &Identity,
    target_device_id: &str,
) -> ConnectResult<(String, String)> {
    let resp = http
        .post(format!("{base}/api/v1/rendezvous"))
        .json(&json!({
            "target_device_id": target_device_id,
            "source_endpoint_id": identity.endpoint_id,
            "source_name": gethostname::gethostname().to_string_lossy(),
        }))
        .send()
        .await
        .map_err(|e| ConnectError::server(format!("rendezvous 发起失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(ConnectError::server(server_message(resp).await));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| ConnectError::server(format!("rendezvous 响应解析失败: {e}")))?;
    let session_id = body["data"]["session_id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| ConnectError::server("rendezvous 响应缺少 session_id"))?;
    let token = body["data"]["rendezvous_token"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| ConnectError::server("rendezvous 响应缺少票据"))?;
    Ok((session_id, token))
}

async fn poll_rendezvous_ready(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    session_id: &str,
) -> ConnectResult<Value> {
    let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
    loop {
        let resp = http
            .get(format!("{base}/api/v1/rendezvous/{session_id}"))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ConnectError::server(format!("rendezvous 状态查询失败: {e}")))?;
        if !resp.status().is_success() {
            return Err(ConnectError::server(server_message(resp).await));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ConnectError::server(format!("rendezvous 状态解析失败: {e}")))?;
        match body["data"]["state"].as_str().unwrap_or_default() {
            "ready" => return Ok(body["data"]["connect"].clone()),
            "rejected" => return Err(ConnectError::session("目标拒绝接入")),
            "unreachable" => return Err(ConnectError::session("目标网络不可达")),
            "expired" | "cancelled" => return Err(ConnectError::session("会话已结束")),
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ConnectError::session("rendezvous 建立超时"));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn login_access_key(
    http: &reqwest::Client,
    proxy_port: u16,
    session_token: &str,
    access_key: &str,
) -> ConnectResult<Role> {
    let resp = http
        .post(format!(
            "http://127.0.0.1:{proxy_port}/api/v1/remote/access-session"
        ))
        .bearer_auth(session_token)
        .json(&json!({ "access_key": access_key }))
        .send()
        .await
        .map_err(|e| ConnectError::session(format!("AccessKey 登录失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(ConnectError::session(server_message(resp).await));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| ConnectError::server(format!("AccessKey 登录响应解析失败: {e}")))?;
    match body["data"]["role"].as_str().unwrap_or_default() {
        "importer" => Ok(Role::Importer),
        "editor" => Ok(Role::Editor),
        "viewer" => Ok(Role::Viewer),
        _ => Err(ConnectError::server("AccessKey 登录响应缺少有效 role")),
    }
}

async fn create_session(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    target_device_id: &str,
    writable: bool,
) -> ConnectResult<String> {
    let resp = http
        .post(format!("{base}/api/v1/sessions"))
        .bearer_auth(key)
        .json(&json!({
            "target_device_id": target_device_id,
            "role": if writable { "importer" } else { "viewer" },
        }))
        .send()
        .await
        .map_err(|e| ConnectError::server(format!("会话发起失败: {e}")))?;
    if !resp.status().is_success() {
        // 目标未开启「允许广域网保存素材」时服务端返 403：降级只读，而不是直接失败
        let denied = writable && resp.status() == reqwest::StatusCode::FORBIDDEN;
        let message = server_message(resp).await;
        return Err(if denied {
            ConnectError::write_denied(message)
        } else {
            ConnectError::server(message)
        });
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| ConnectError::server(format!("会话响应解析失败: {e}")))?;
    if body["data"]["state"] == "rejected" {
        // 写请求被目标拒（本机开关与心跳上报存在窗口差）同样降级重试；只读请求被拒是真拒绝
        return Err(if writable {
            ConnectError::write_denied("对方设备未开启远程写入")
        } else {
            ConnectError::session("对方设备已关闭远程访问")
        });
    }
    body["data"]["session_id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| ConnectError::server("会话响应缺少 session_id"))
}

/// 轮询会话状态直到 ready/终态；返回 connect 对象（endpoint_id + 直连地址 + 可选中继）
async fn poll_ready(
    http: &reqwest::Client,
    base: &str,
    key: &str,
    session_id: &str,
    writable: bool,
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
            "rejected" => {
                return Err(if writable {
                    ConnectError::write_denied("对方未开启远程写入")
                } else {
                    ConnectError::session("对方拒绝")
                })
            }
            "unreachable" => {
                return Err(ConnectError::session(
                    "目标网络不可达：服务端未提供中继且目标无直连地址",
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

async fn cancel_rendezvous(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    session_id: &str,
) -> Result<(), String> {
    http.delete(format!("{base}/api/v1/rendezvous/{session_id}"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("rendezvous 取消失败: {e}"))?;
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

fn random_session_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("系统随机源可用");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

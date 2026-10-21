//! B 侧会话编排：发起会话 → 轮询 ready → 候选依序连接（指纹钉扎）→ 拉起会话代理。
//! 单活动会话模型（查看方同时只看一个远端库）：新会话自动顶替并断开旧会话。
//! 会话生命周期经 SSE `remote.session` 事件推送（app 断线回切依赖）。

use crate::core::events::EventBus;
use crate::remote::data::{serve_proxy, ProxyContext};
use crate::remote::identity::Identity;
use serde::Serialize;
use serde_json::{json, Value};
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

struct ActiveSession {
    dto: ActiveSessionDto,
    /// QUIC endpoint 须随连接持有（drop 即断开）
    _endpoint: quinn::Endpoint,
    conn: quinn::Connection,
    proxy_shutdown: tokio::sync::oneshot::Sender<()>,
    /// 中继会话的 WS 驱动（中止即关闭中继通道；直连为 None）
    relay_driver: Option<tokio::task::JoinHandle<()>>,
}

pub struct SessionManager {
    active: Mutex<Option<ActiveSession>>,
    bus: EventBus,
}

impl SessionManager {
    pub fn new(bus: EventBus) -> Arc<SessionManager> {
        Arc::new(SessionManager {
            active: Mutex::new(None),
            bus,
        })
    }

    pub fn snapshot(&self) -> Option<ActiveSessionDto> {
        self.active.lock().unwrap().as_ref().map(|s| s.dto.clone())
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

        // ---- 建立数据面：relay 经服务端中继 WS；direct 依序尝试候选 ----
        let (endpoint, conn, relay_driver) = if mode == "relay" {
            let ws_url =
                crate::remote::relay::relay_ws_url(base, key, &self_device_id, &session_id, "b")
                    .map_err(ConnectError::server)?;
            let (ws, _) = tokio_tungstenite::connect_async(&ws_url)
                .await
                .map_err(|e| ConnectError::server(format!("中继通道连接失败: {e}")))?;
            let (endpoint, conn, driver) =
                crate::remote::relay::connect_over_ws(identity, ws, expected)
                    .await
                    .map_err(ConnectError::session)?;
            (endpoint, conn, Some(driver))
        } else {
            let candidates = connect_info["candidates"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let mut last_err = "无可用候选".to_string();
            let mut connected = None;
            for candidate in &candidates {
                let (Some(host), Some(port)) = (
                    candidate["host"].as_str(),
                    candidate["port"].as_u64().map(|p| p as u16),
                ) else {
                    continue;
                };
                match crate::remote::data::DataPlane::connect(identity, host, port, expected).await
                {
                    Ok((endpoint, conn)) => {
                        connected = Some((endpoint, conn));
                        break;
                    }
                    Err(e) => {
                        tracing::debug!("候选连接失败: {e}");
                        last_err = e;
                    }
                }
            }
            let Some((endpoint, conn)) = connected else {
                let _ = cancel_session(http, base, key, &session_id).await;
                return Err(ConnectError::session(format!("直连失败：{last_err}")));
            };
            (endpoint, conn, None)
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
        let ctx = Arc::new(ProxyContext {
            conn: conn.clone(),
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
            _endpoint: endpoint,
            conn: conn.clone(),
            proxy_shutdown: shutdown_tx,
            relay_driver,
        });
        if let Some(old) = old {
            let old_session_id = old.dto.session_id.clone();
            old.teardown();
            let _ = cancel_session(http, base, key, &old_session_id).await;
        }

        // ---- 断线监控：隧道断开 → 本地收尾 + SSE 通知（app 断线回切） ----
        {
            let manager = self.clone();
            let session_id = dto.session_id.clone();
            let target_device_id = dto.target_device_id.clone();
            tokio::spawn(async move {
                conn.closed().await;
                if manager.clear_if_current(&session_id) {
                    tracing::info!("远程会话 {session_id} 隧道已断开");
                    manager.publish(&session_id, &target_device_id, "disconnected");
                }
            });
        }

        self.publish(&dto.session_id, &dto.target_device_id, "connected");
        Ok(dto)
    }

    /// 断开当前会话（幂等）：通知服务端取消 + 本地收尾
    pub async fn disconnect(&self, http: &reqwest::Client, base: &str, key: &str) {
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
    /// 主动收尾：断隧道 + 停代理 + 关中继通道
    fn teardown(self) {
        let _ = self.proxy_shutdown.send(());
        self.conn.close(0u32.into(), b"session closed");
        if let Some(driver) = self.relay_driver {
            driver.abort();
        }
    }

    /// 断线监控收尾：连接已死，只停代理与中继通道
    fn teardown_silent(self) {
        let _ = self.proxy_shutdown.send(());
        if let Some(driver) = self.relay_driver {
            driver.abort();
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

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("系统随机源可用");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

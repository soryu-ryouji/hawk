//! 远程访问客户端模块（可选子系统）：信令（注册/心跳/WSS）与数据面（QUIC，阶段 1b）。
//! 应用级 `remote.toml` 启用才激活；supervisor reconcile 管理（同 LAN web 先例）——
//! 配置变更热重启信令，连接失败只写状态不崩进程。设计见 docs/backend/remote-access.md。

pub mod config;
pub mod data;
pub mod identity;
pub mod keys;
pub mod mux;
pub mod reachability;
pub mod relay;
pub mod session;
pub mod signal;
pub mod tls;

use crate::core::config::LibraryConfig;
use crate::core::events::EventBus;
use crate::core::paths::LibraryPaths;
use crate::remote::config::{RemoteConfigStore, RemoteSettings};
use serde::Serialize;
use std::sync::{Arc, Mutex};

/// SSE 事件名（信令状态变化推送，负载即 RemoteStatus）
pub const REMOTE_STATUS_EVENT: &str = "remote.status";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteState {
    /// 未启用或配置不完整（任务不 spawn、无网络活动）
    #[default]
    Disabled,
    /// 接入中（注册/重连进行中）
    Connecting,
    /// 控制通道已连接（在线，可被其他设备发现与会话）
    Online,
    /// 致命错误（key 无效、url 非法等），等配置变更重启
    Error,
}

/// 本地库上下文（注册/心跳上报库名与可访问性；远程查看模式无本地库 → None，
/// 设备以「远程查看」名义上线且不可被访问）
#[derive(Clone)]
pub struct LibraryContext {
    pub paths: LibraryPaths,
    pub config: Arc<LibraryConfig>,
}

#[derive(Clone, Debug, Default, Serialize, utoipa::ToSchema)]
pub struct RemoteStatus {
    pub state: RemoteState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// remote supervisor：期望态（enabled && url/service_key 非空 → 信令运行中）与实际任务的 reconcile
pub struct Supervisor {
    notify: tokio::sync::Notify,
    status: Mutex<RemoteStatus>,
    bus: EventBus,
    /// B 侧会话编排（单活动会话；生命周期事件经 SSE 推送）
    pub sessions: Arc<session::SessionManager>,
    /// 访问密钥（hawk-app key）本地签发与保管（查看者会话的本地执法依据）
    pub keys: Arc<keys::KeyStore>,
    /// 应用级远程配置（接入参数跨素材库共享；身份目录由配置位置派生）
    config: Arc<RemoteConfigStore>,
}

impl Supervisor {
    pub fn new(bus: EventBus, config: Arc<RemoteConfigStore>) -> Arc<Supervisor> {
        Arc::new(Supervisor {
            notify: tokio::sync::Notify::new(),
            status: Mutex::new(RemoteStatus::default()),
            sessions: session::SessionManager::new(bus.clone(), config.clone()),
            keys: keys::KeyStore::open(config.identity_dir()),
            bus,
            config,
        })
    }

    pub fn remote_config(&self) -> &Arc<RemoteConfigStore> {
        &self.config
    }

    /// 唤醒收敛（watcher 配置变更回调同步调用；Notify 无等待者时记 permit，不丢事件）
    pub fn wake(&self) {
        self.notify.notify_one();
    }

    /// 当前状态快照（api/remote 序列化给前端）
    pub fn snapshot(&self) -> RemoteStatus {
        self.status.lock().unwrap().clone()
    }

    pub(crate) fn set_status(&self, status: RemoteStatus) {
        *self.status.lock().unwrap() = status.clone();
        // 信令状态变化经 SSE 推送（app 免轮询）
        self.bus
            .publish(REMOTE_STATUS_EVENT, serde_json::json!(status));
    }

    /// 常驻任务：唤醒 → 收敛 → 等待下一轮。router 为 daemon API 路由克隆（A 侧隧道端进程内转发目标）。
    /// library 为本地库上下文（设备身份是应用级，切换库后心跳会同步库名）；None = 远程查看模式
    pub async fn run(self: Arc<Self>, library: Option<LibraryContext>, router: axum::Router) {
        let mut current: Option<(RemoteSettings, tokio::task::JoinHandle<()>)> = None;
        loop {
            let cfg = self.config.current();
            // 写权限是请求级热配置（auth 每请求读 current），不触发信令/数据面重启；
            // 只有接入参数变化才需要重建连接。
            let need_restart = match current.as_ref() {
                Some((active, _)) => !same_connection(active, &cfg),
                None => activatable(&cfg),
            };
            if need_restart {
                if let Some((_, handle)) = current.take() {
                    handle.abort();
                    // 等旧任务真正结束：数据面的 Drop 守卫在此时才关停 QUIC 端点，
                    // 不等就会出现「新任务绑定自己刚占用的端口」→ EADDRINUSE
                    let _ = handle.await;
                }
                current = if activatable(&cfg) {
                    tracing::info!("远程访问信令启动: {}", cfg.url);
                    self.set_status(RemoteStatus {
                        state: RemoteState::Connecting,
                        server_url: Some(cfg.url.clone()),
                        device_id: None,
                        error: None,
                    });
                    let handle = tokio::spawn(signal::run(
                        library.clone(),
                        self.config.clone(),
                        cfg.clone(),
                        self.clone(),
                        router.clone(),
                    ));
                    Some((cfg, handle))
                } else {
                    if cfg.enabled {
                        tracing::warn!("远程访问已启用但缺少 url 或 key，远程访问未启动");
                        self.set_status(RemoteStatus {
                            state: RemoteState::Error,
                            server_url: None,
                            device_id: None,
                            error: Some("已启用但 url 或 key 未配置".to_string()),
                        });
                    } else {
                        self.set_status(RemoteStatus::default());
                    }
                    None
                };
            } else if let Some((active, _)) = current.as_mut() {
                // 仅 writable 等非连接字段变化：保留运行中的信令/数据面
                *active = cfg;
            }
            self.notify.notified().await;
        }
    }
}

fn activatable(cfg: &RemoteSettings) -> bool {
    cfg.enabled && !cfg.url.is_empty() && !cfg.service_key.is_empty()
}

/// 是否需要重建信令/数据面：写权限等运行期配置不算连接参数
fn same_connection(a: &RemoteSettings, b: &RemoteSettings) -> bool {
    a.enabled == b.enabled
        && a.url == b.url
        && a.service_key == b.service_key
        && a.quic_port == b.quic_port
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_change_publishes_sse_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let dir = tempfile::tempdir().unwrap();
        let paths = LibraryPaths::new(dir.path().to_str().unwrap(), None);
        let config = RemoteConfigStore::open(None, Some(&paths));
        let supervisor = Supervisor::new(bus, config);
        supervisor.set_status(RemoteStatus {
            state: RemoteState::Online,
            server_url: Some("https://example.com".to_string()),
            device_id: Some("d_1".to_string()),
            error: None,
        });
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("事件超时")
            .expect("事件接收");
        assert_eq!(event.kind, REMOTE_STATUS_EVENT);
        assert_eq!(event.payload["state"], "online");
        assert_eq!(event.payload["device_id"], "d_1");
    }
}

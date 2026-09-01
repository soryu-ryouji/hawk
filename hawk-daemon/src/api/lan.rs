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

use super::SharedState;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 排空在途连接的上限：超时强杀（局域网查看器可自行重连，无零中断要求）
const DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// LAN 监听状态快照（app/info 序列化 + 日志）
#[derive(Clone, Default)]
pub struct LanStatus {
    pub active: bool,
    pub port: Option<u16>,
    pub error: Option<String>,
}

pub struct LanSupervisor {
    notify: tokio::sync::Notify,
    status: Mutex<LanStatus>,
}

impl LanSupervisor {
    pub fn new() -> Arc<LanSupervisor> {
        Arc::new(LanSupervisor {
            notify: tokio::sync::Notify::new(),
            status: Mutex::new(LanStatus::default()),
        })
    }

    /// 唤醒收敛（watcher 回调同步调用；Notify 无等待者时记 permit，不丢事件）
    pub fn wake(&self) {
        self.notify.notify_one();
    }

    /// 当前状态快照
    pub fn snapshot(&self) -> LanStatus {
        self.status.lock().unwrap().clone()
    }

    fn set_status(&self, status: LanStatus) {
        *self.status.lock().unwrap() = status;
    }

    /// 常驻任务：唤醒 → 收敛 → 等待下一轮
    pub async fn run(self: Arc<Self>, state: SharedState) {
        // (端口, serve 任务, 优雅关停信号)
        let mut current: Option<(u16, tokio::task::JoinHandle<()>, tokio::sync::oneshot::Sender<()>)> = None;
        loop {
            let web = state.config.current().web;
            let desired = if web.enabled && web.token.is_some() {
                Some(web.port)
            } else {
                if web.enabled && web.token.is_none() {
                    tracing::warn!("[web] enabled 但缺少 token，局域网查看未启动（在设置面板配置 token）");
                }
                None
            };

            let bound = current.as_ref().map(|(port, _, _)| *port);
            if bound != desired {
                // 拆旧：发关停信号排空在途请求，超时强杀（新端口可能就是旧端口，必须先释放）
                if let Some((_, handle, shutdown)) = current.take() {
                    let _ = shutdown.send(());
                    if tokio::time::timeout(DRAIN_TIMEOUT, handle).await.is_err() {
                        tracing::warn!("局域网旧监听 {DRAIN_TIMEOUT:?} 内未排空，强制结束");
                    }
                }
                match desired {
                    Some(port) => match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                        Ok(listener) => {
                            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
                            let app = crate::api::build_router(state.clone());
                            let handle = tokio::spawn(async move {
                                if let Err(e) = axum::serve(listener, app)
                                    .with_graceful_shutdown(async move {
                                        let _ = shutdown_rx.await;
                                    })
                                    .await
                                {
                                    tracing::error!("局域网监听退出: {e}");
                                }
                            });
                            current = Some((port, handle, shutdown_tx));
                            tracing::info!("局域网查看已启动: 0.0.0.0:{port}");
                            self.set_status(LanStatus {
                                active: true,
                                port: Some(port),
                                error: None,
                            });
                        }
                        Err(e) => {
                            tracing::error!("局域网端口 {port} 绑定失败: {e}");
                            self.set_status(LanStatus {
                                active: false,
                                port: None,
                                error: Some(format!("端口 {port} 绑定失败: {e}")),
                            });
                        }
                    },
                    None => {
                        tracing::info!("局域网查看已停止");
                        self.set_status(LanStatus::default());
                    }
                }
            }
            self.notify.notified().await;
        }
    }
}

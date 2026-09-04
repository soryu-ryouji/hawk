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
use super::{AccessLevel, SharedState};
use crate::core::config::WebSettings;
use axum::extract::{Extension, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

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
    /// (收敛代数, 状态)：每轮收敛完成（含 no-op 轮）代数 +1，
    /// PUT app/lan 据此等待「本轮」结果，避免读到 wake 之前的旧状态误判
    status: Mutex<(u64, LanStatus)>,
}

impl LanSupervisor {
    pub fn new() -> Arc<LanSupervisor> {
        Arc::new(LanSupervisor {
            notify: tokio::sync::Notify::new(),
            status: Mutex::new((0, LanStatus::default())),
        })
    }

    /// 唤醒收敛（watcher 回调同步调用；Notify 无等待者时记 permit，不丢事件）
    pub fn wake(&self) {
        self.notify.notify_one();
    }

    /// 当前状态快照
    pub fn snapshot(&self) -> LanStatus {
        self.status.lock().unwrap().1.clone()
    }

    /// 当前收敛代数与状态快照
    fn snapshot_epoch(&self) -> (u64, LanStatus) {
        let guard = self.status.lock().unwrap();
        (guard.0, guard.1.clone())
    }

    fn set_status(&self, status: LanStatus) {
        self.status.lock().unwrap().1 = status;
    }

    /// 本轮收敛完成：推进代数（PUT app/lan 的等待出口）
    fn bump_epoch(&self) {
        self.status.lock().unwrap().0 += 1;
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
            self.bump_epoch();
            self.notify.notified().await;
        }
    }
}

// ---------- app/lan 端点：LAN 查看配置的读写（admin 限定） ----------
// 配置由 daemon 权威读写（toml_edit 保留注释），保存即热重绑：写配置 → reload → wake →
// 等待本轮收敛（epoch），绑定失败回滚旧配置。不再经 Electron 主进程手写 TOML + 轮询 app/info。

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new().routes(routes!(get_lan, put_lan))
}

#[derive(Serialize, utoipa::ToSchema)]
struct LanSettingsDto {
    enabled: bool,
    port: u16,
    token: String,
    writable: bool,
    separate_write_token: bool,
    write_token: String,
    /// 运行状态（热重绑实况）：active=监听中，error=绑定失败原因
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn lan_dto(state: &SharedState) -> LanSettingsDto {
    let web = state.config.current().web;
    let status = state.lan.snapshot();
    LanSettingsDto {
        enabled: web.enabled,
        port: web.port,
        token: web.token.unwrap_or_default(),
        writable: web.writable,
        separate_write_token: web.separate_write_token,
        write_token: web.write_token.unwrap_or_default(),
        active: status.active,
        error: status.error,
    }
}

/// 仅 admin 可读写 LAN 配置：viewer（含可写）不应看到 token 字段，否则只读 token 可提权为可写
fn require_admin(access: &AccessLevel) -> Result<(), ApiError> {
    if matches!(access, AccessLevel::Admin) {
        Ok(())
    } else {
        Err(ApiError::new(
            codes::READ_ONLY,
            axum::http::StatusCode::FORBIDDEN,
            "viewer token cannot access lan settings",
        ))
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
    Ok(Json(Envelope::ok(lan_dto(&state))))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct PutLanBody {
    enabled: bool,
    port: u16,
    #[serde(default)]
    token: Option<String>,
    writable: bool,
    #[serde(default)]
    separate_write_token: bool,
    #[serde(default)]
    write_token: Option<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// 期望监听端口（与 supervisor 判定一致）：enabled 且 token 非空才应绑定
fn desired_port(web: &WebSettings) -> Option<u16> {
    if web.enabled && web.token.is_some() {
        Some(web.port)
    } else {
        None
    }
}

/// 等待 supervisor 完成一轮收敛并达到期望态；绑定失败返回错误原因。
/// epoch 机制保证判定基于 wake 之后的新一轮收敛，而非残留旧状态。
async fn wait_converged(state: &SharedState, web: &WebSettings, epoch0: u64) -> Result<(), String> {
    let desired = desired_port(web);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (epoch, status) = state.lan.snapshot_epoch();
        if epoch > epoch0 {
            if let Some(err) = status.error {
                return Err(err);
            }
            let converged = match desired {
                Some(port) => status.active && status.port == Some(port),
                None => !status.active,
            };
            if converged {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("局域网设置生效超时（daemon 未完成监听重绑）".to_string());
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
    let new_web = WebSettings {
        enabled: body.enabled,
        port: body.port,
        token: non_empty(body.token),
        writable: body.writable,
        separate_write_token: body.separate_write_token,
        write_token: non_empty(body.write_token),
    };
    if new_web.port == 0 {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "端口须为 1–65535 之间的数字",
        ));
    }
    if new_web.enabled && new_web.token.is_none() {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "启用局域网查看需要填写访问 token",
        ));
    }
    if new_web.enabled && new_web.writable && new_web.separate_write_token && new_web.write_token.is_none() {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "拆分只读/可写 token 需要填写可写 token",
        ));
    }

    let old_web = state.config.current().web;
    let epoch0 = state.lan.snapshot_epoch().0;
    state
        .config
        .update_web(&new_web)
        .map_err(|e| ApiError::new(codes::INTERNAL, axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.lan.wake();

    if let Err(err) = wait_converged(&state, &new_web, epoch0).await {
        // 失败回滚：写回旧配置，走同一条热更路径收敛回旧态（尽力，结果不敏感）
        if state.config.update_web(&old_web).is_ok() {
            let epoch1 = state.lan.snapshot_epoch().0;
            state.lan.wake();
            let _ = wait_converged(&state, &old_web, epoch1).await;
        }
        return Err(ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("局域网监听未生效：{err}（已回滚原配置）"),
        ));
    }
    Ok(Json(Envelope::ok(lan_dto(&state))))
}

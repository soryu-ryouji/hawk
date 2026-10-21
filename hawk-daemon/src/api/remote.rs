//! 远程访问端点（admin 限定）：接入配置读写与运行状态。配置写回 config.toml [remote] 段，
//! 保存即热生效（remote supervisor 经 wake 热重启信令），模型与 app/lan 一致。

use super::envelope::{codes, ApiError, Envelope, SuccessOnly};
use super::{require_admin, AccessLevel, SharedState};
use crate::core::config::RemoteSettings;
use crate::remote::session::ActiveSessionDto;
use crate::remote::RemoteStatus;
use axum::extract::State;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(get_remote, put_remote))
        .routes(routes!(list_devices))
        .routes(routes!(get_session, create_session, delete_session))
        .routes(routes!(patch_device, rotate_device))
}

#[derive(Serialize, utoipa::ToSchema)]
struct RemoteDto {
    enabled: bool,
    url: String,
    key: String,
    /// 运行状态（信令实况）：connecting=接入中，online=控制通道已连接，error=致命错误原因
    status: RemoteStatus,
}

fn remote_dto(state: &SharedState) -> RemoteDto {
    let cfg = state.config.current().remote;
    RemoteDto {
        enabled: cfg.enabled,
        url: cfg.url,
        key: cfg.key,
        status: state.remote.snapshot(),
    }
}

/// 读取远程访问接入配置与运行状态（admin 限定；viewer 403，防止只读 token 看到接入信息）
#[utoipa::path(
    get,
    path = "/api/v1/remote",
    tags = ["remote"],
    responses((status = 200, description = "OK", body = Envelope<RemoteDto>))
)]
async fn get_remote(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
) -> Result<Json<Envelope<RemoteDto>>, ApiError> {
    require_admin(&access)?;
    Ok(Json(Envelope::ok(remote_dto(&state))))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct PutRemoteBody {
    enabled: bool,
    url: String,
    key: String,
}

/// 写回 .hawk/config.toml 的 [remote] 段并热重启信令（admin 限定）。
/// 接入是异步过程（注册/重连由 supervisor 驱动），本端点写完配置即返回，进展经 GET 轮询状态
#[utoipa::path(
    put,
    path = "/api/v1/remote",
    tags = ["remote"],
    request_body = PutRemoteBody,
    responses((status = 200, description = "OK", body = Envelope<RemoteDto>))
)]
async fn put_remote(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
    Json(body): Json<PutRemoteBody>,
) -> Result<Json<Envelope<RemoteDto>>, ApiError> {
    require_admin(&access)?;
    // quic_port 不在 PUT 体（高级项，手编 config.toml 调整）：保留现值
    let new_remote = RemoteSettings {
        enabled: body.enabled,
        url: body.url.trim().trim_end_matches('/').to_string(),
        key: body.key.trim().to_string(),
        quic_port: state.config.current().remote.quic_port,
    };
    if new_remote.enabled {
        if new_remote.url.is_empty() || new_remote.key.is_empty() {
            return Err(ApiError::new(
                codes::INVALID_PARAM,
                axum::http::StatusCode::BAD_REQUEST,
                "启用远程访问需要填写服务端地址与访问密钥",
            ));
        }
        if !new_remote.url.starts_with("http://") && !new_remote.url.starts_with("https://") {
            return Err(ApiError::new(
                codes::INVALID_PARAM,
                axum::http::StatusCode::BAD_REQUEST,
                "服务端地址需以 http:// 或 https:// 开头",
            ));
        }
    }
    state.config.update_remote(&new_remote).map_err(|e| {
        ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e,
        )
    })?;
    state.remote.wake();
    Ok(Json(Envelope::ok(remote_dto(&state))))
}

// ---------- 设备列表与会话（admin 限定；转发自 hawk-server，key 不出本进程） ----------

/// 取接入配置并构建信令 HTTP 客户端；未配置返回 400
fn signal_client(state: &SharedState) -> Result<(reqwest::Client, String, String), ApiError> {
    let cfg = state.config.current().remote;
    if cfg.url.is_empty() || cfg.key.is_empty() {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "未配置远程接入（先 PUT /api/v1/remote 设置服务端地址与密钥）",
        ));
    }
    let base = cfg.url.trim_end_matches('/').to_string();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| {
            ApiError::new(
                codes::INTERNAL,
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("HTTP 客户端构建失败: {e}"),
            )
        })?;
    Ok((client, base, cfg.key))
}

fn bad_gateway(message: String) -> ApiError {
    ApiError::new(
        codes::REMOTE_UPSTREAM,
        axum::http::StatusCode::BAD_GATEWAY,
        message,
    )
}

#[derive(Serialize, utoipa::ToSchema)]
struct RemoteDeviceDto {
    device_id: String,
    name: String,
    platform: String,
    library_name: String,
    online: bool,
    allow_remote: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_at_ms: Option<u64>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct RemoteDevicesDto {
    devices: Vec<RemoteDeviceDto>,
}

/// 拉取服务端设备列表（admin 限定）。本机设备与对端设备同列，app 据 online/allow_remote 展示
#[utoipa::path(
    get,
    path = "/api/v1/remote/devices",
    tags = ["remote"],
    responses((status = 200, description = "OK", body = Envelope<RemoteDevicesDto>))
)]
async fn list_devices(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
) -> Result<Json<Envelope<RemoteDevicesDto>>, ApiError> {
    require_admin(&access)?;
    let (http, base, key) = signal_client(&state)?;
    let resp = http
        .get(format!("{base}/api/v1/devices"))
        .bearer_auth(&key)
        .send()
        .await
        .map_err(|e| bad_gateway(format!("服务端不可达: {e}")))?;
    if !resp.status().is_success() {
        return Err(bad_gateway(server_message(resp).await));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| bad_gateway(format!("服务端响应解析失败: {e}")))?;
    let devices = body["data"]["devices"]
        .as_array()
        .map(|devices| {
            devices
                .iter()
                .map(|d| RemoteDeviceDto {
                    device_id: d["device_id"].as_str().unwrap_or_default().to_string(),
                    name: d["name"].as_str().unwrap_or_default().to_string(),
                    platform: d["platform"].as_str().unwrap_or_default().to_string(),
                    library_name: d["library_name"].as_str().unwrap_or_default().to_string(),
                    online: d["online"].as_bool().unwrap_or(false),
                    allow_remote: d["allow_remote"].as_bool().unwrap_or(false),
                    last_seen_at_ms: d["last_seen_at_ms"].as_u64(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(Envelope::ok(RemoteDevicesDto { devices })))
}

/// 读取当前活动远程会话（无活动会话时 data 为 null）
#[utoipa::path(
    get,
    path = "/api/v1/remote/session",
    tags = ["remote"],
    responses((status = 200, description = "OK", body = Envelope<Option<ActiveSessionDto>>))
)]
async fn get_session(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
) -> Result<Json<Envelope<Option<ActiveSessionDto>>>, ApiError> {
    require_admin(&access)?;
    Ok(Json(Envelope::ok(state.remote.sessions.snapshot())))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct CreateSessionBody {
    target_device_id: String,
}

/// 建立到目标设备的远程会话：发起 → 轮询 ready → 候选依序连接 → 拉起会话代理。
/// 单活动会话模型：新会话自动顶替旧会话。返回代理连接信息（api_base + token + 展示名）
#[utoipa::path(
    post,
    path = "/api/v1/remote/session",
    tags = ["remote"],
    request_body = CreateSessionBody,
    responses((status = 200, description = "OK", body = Envelope<ActiveSessionDto>))
)]
async fn create_session(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
    Json(body): Json<CreateSessionBody>,
) -> Result<Json<Envelope<ActiveSessionDto>>, ApiError> {
    require_admin(&access)?;
    let (http, base, key) = signal_client(&state)?;
    let identity = crate::remote::identity::Identity::load_or_create(&state.paths.remote_dir)
        .map_err(|e| {
            ApiError::new(
                codes::INTERNAL,
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e,
            )
        })?;
    let dto = state
        .remote
        .sessions
        .connect(&http, &base, &key, &identity, &body.target_device_id)
        .await
        .map_err(|e| {
            if e.is_server {
                bad_gateway(e.message)
            } else {
                ApiError::new(
                    codes::REMOTE_SESSION_FAILED,
                    axum::http::StatusCode::CONFLICT,
                    e.message,
                )
            }
        })?;
    Ok(Json(Envelope::ok(dto)))
}

/// 断开当前远程会话（幂等）：通知服务端取消、断开隧道、关闭会话代理
#[utoipa::path(
    delete,
    path = "/api/v1/remote/session",
    tags = ["remote"],
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn delete_session(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
) -> Result<Json<super::envelope::SuccessOnly>, ApiError> {
    require_admin(&access)?;
    let (http, base, key) = signal_client(&state)?;
    state.remote.sessions.disconnect(&http, &base, &key).await;
    Ok(super::envelope::success())
}

// ---------- 设备管理（admin 限定；转发服务端 PATCH /devices/{self}） ----------

#[derive(Deserialize, utoipa::ToSchema)]
struct PatchDeviceBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    allow_remote: Option<bool>,
    #[serde(default)]
    library_name: Option<String>,
}

/// 更新本机设备在服务端的信息：显示名 / 远程访问开关 / 库名（至少一项）
#[utoipa::path(
    patch,
    path = "/api/v1/remote/device",
    tags = ["remote"],
    request_body = PatchDeviceBody,
    responses((status = 200, description = "OK", body = Envelope<RemoteDeviceDto>))
)]
async fn patch_device(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
    Json(body): Json<PatchDeviceBody>,
) -> Result<Json<Envelope<RemoteDeviceDto>>, ApiError> {
    require_admin(&access)?;
    if body.name.is_none() && body.allow_remote.is_none() && body.library_name.is_none() {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "至少提供一项待更新字段",
        ));
    }
    let (http, base, key) = signal_client(&state)?;
    let device_id = registered_device_id(&state)?;
    let mut payload = serde_json::Map::new();
    if let Some(name) = body.name {
        payload.insert("name".to_string(), Value::from(name));
    }
    if let Some(allow_remote) = body.allow_remote {
        payload.insert("allow_remote".to_string(), Value::from(allow_remote));
    }
    if let Some(library_name) = body.library_name {
        payload.insert("library_name".to_string(), Value::from(library_name));
    }
    let resp = http
        .patch(format!("{base}/api/v1/devices/{device_id}"))
        .bearer_auth(&key)
        .json(&Value::Object(payload))
        .send()
        .await
        .map_err(|e| bad_gateway(format!("服务端不可达: {e}")))?;
    if !resp.status().is_success() {
        return Err(bad_gateway(server_message(resp).await));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| bad_gateway(format!("服务端响应解析失败: {e}")))?;
    let d = &body["data"];
    Ok(Json(Envelope::ok(RemoteDeviceDto {
        device_id: d["device_id"].as_str().unwrap_or_default().to_string(),
        name: d["name"].as_str().unwrap_or_default().to_string(),
        platform: d["platform"].as_str().unwrap_or_default().to_string(),
        library_name: d["library_name"].as_str().unwrap_or_default().to_string(),
        online: d["online"].as_bool().unwrap_or(false),
        allow_remote: d["allow_remote"].as_bool().unwrap_or(false),
        last_seen_at_ms: d["last_seen_at_ms"].as_u64(),
    })))
}

#[derive(Serialize, utoipa::ToSchema)]
struct RotateDeviceDto {
    /// 轮换后的新证书指纹（TOFU 锚点，旧证书会话随即失效）
    fingerprint: String,
}

/// 证书轮换：重新生成自签证书，同步服务端指纹并落盘，信令热重启（旧证书会话失效）
#[utoipa::path(
    post,
    path = "/api/v1/remote/device/rotate",
    tags = ["remote"],
    responses((status = 200, description = "OK", body = Envelope<RotateDeviceDto>))
)]
async fn rotate_device(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
) -> Result<Json<Envelope<RotateDeviceDto>>, ApiError> {
    require_admin(&access)?;
    let (http, base, key) = signal_client(&state)?;
    let mut identity = crate::remote::identity::Identity::load_or_create(&state.paths.remote_dir)
        .map_err(|e| {
        ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e,
        )
    })?;
    if identity.device_id.is_none() {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "设备未注册（先启用远程接入并完成注册）",
        ));
    }
    identity.rotate_cert().map_err(|e| {
        ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e,
        )
    })?;
    // 先同步服务端（新指纹生效），再落盘重启信令
    let device_id = identity.device_id.clone().unwrap();
    let resp = http
        .patch(format!("{base}/api/v1/devices/{device_id}"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "fingerprint": identity.fingerprint }))
        .send()
        .await
        .map_err(|e| bad_gateway(format!("服务端不可达: {e}")))?;
    if !resp.status().is_success() {
        return Err(bad_gateway(server_message(resp).await));
    }
    identity.persist(&state.paths.remote_dir).map_err(|e| {
        ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e,
        )
    })?;
    state.remote.wake();
    Ok(Json(Envelope::ok(RotateDeviceDto {
        fingerprint: identity.fingerprint,
    })))
}

/// 本机已注册的 device_id（未注册 → 400）
fn registered_device_id(state: &SharedState) -> Result<String, ApiError> {
    let identity = crate::remote::identity::Identity::load_or_create(&state.paths.remote_dir)
        .map_err(|e| {
            ApiError::new(
                codes::INTERNAL,
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e,
            )
        })?;
    identity.device_id.ok_or_else(|| {
        ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "设备未注册（先启用远程接入并完成注册）",
        )
    })
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

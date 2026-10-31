//! 远程访问端点（admin 限定）：接入配置读写与运行状态。配置写回应用级 remote.toml，
//! 保存即热生效（remote supervisor 经 wake 热重启信令），模型与 app/lan 一致。

use super::envelope::{codes, ApiError, Envelope, SuccessOnly};
use super::{require_manage, AccessContext, SharedState};
use crate::core::access::Role;
use crate::remote::config::RemoteSettings;
use crate::remote::session::ActiveSessionDto;
use crate::remote::RemoteStatus;
use axum::extract::State;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub(crate) const ACCESS_SESSION_PATH: &str = "/api/v1/remote/access-session";

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(authenticate_access_session))
        .routes(routes!(renew_lease))
        .routes(routes!(get_remote, put_remote))
        .routes(routes!(list_devices))
        .routes(routes!(create_access_session))
        .routes(routes!(get_session, create_session, delete_session))
        .routes(routes!(patch_device, rotate_device, delete_device))
        .routes(routes!(list_keys, create_key))
        .routes(routes!(patch_key, delete_key))
}

#[derive(Serialize, utoipa::ToSchema)]
struct RemoteDto {
    enabled: bool,
    url: String,
    /// 服务端 tenant-key（池门禁凭证，hawk-server tenant add 生成）
    tenant_key: String,
    /// 允许广域网会话导入素材/新建文件夹（与 [web] 的局域网写权限独立）
    allow_import: bool,
    /// 使用中继（服务端提供时；消耗自建带宽，设备级开关）
    relay: bool,
    /// 运行状态（信令实况）：connecting=接入中，online=控制通道已连接，error=致命错误原因
    status: RemoteStatus,
}

fn remote_dto(state: &SharedState) -> RemoteDto {
    let cfg = state.remote.remote_config().current();
    RemoteDto {
        enabled: cfg.enabled,
        url: cfg.url,
        tenant_key: cfg.tenant_key,
        allow_import: cfg.allow_import,
        relay: cfg.relay,
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
    Extension(access): Extension<AccessContext>,
) -> Result<Json<Envelope<RemoteDto>>, ApiError> {
    require_manage(&access)?;
    Ok(Json(Envelope::ok(remote_dto(&state))))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct PutRemoteBody {
    enabled: bool,
    url: String,
    /// 服务端 tenant-key
    tenant_key: String,
    /// 允许广域网会话导入素材/新建文件夹；省略时按关闭处理
    #[serde(default)]
    allow_import: bool,
    /// 使用中继（服务端提供时）；省略时保留现值
    #[serde(default)]
    relay: Option<bool>,
}

/// 写回应用级 remote.toml 并热重启信令（admin 限定）。
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
    Extension(access): Extension<AccessContext>,
    Json(body): Json<PutRemoteBody>,
) -> Result<Json<Envelope<RemoteDto>>, ApiError> {
    require_manage(&access)?;
    // quic_port 不在 PUT 体（高级项，编辑 remote.toml 后重启 daemon 调整）：保留现值
    let new_remote = RemoteSettings {
        enabled: body.enabled,
        url: body.url.trim().trim_end_matches('/').to_string(),
        tenant_key: body.tenant_key.trim().to_string(),
        quic_port: state.remote.remote_config().current().quic_port,
        allow_import: body.allow_import,
        // 省略时保留现值（设置面板整体保存时会显式携带）
        relay: body
            .relay
            .unwrap_or_else(|| state.remote.remote_config().current().relay),
    };
    if new_remote.enabled {
        if new_remote.url.is_empty() || new_remote.tenant_key.is_empty() {
            return Err(ApiError::new(
                codes::INVALID_PARAM,
                axum::http::StatusCode::BAD_REQUEST,
                "启用远程访问需要填写服务端地址与服务密钥（tenant-key）",
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
    state
        .remote
        .remote_config()
        .update(&new_remote)
        .map_err(|e| {
            ApiError::new(
                codes::INTERNAL,
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e,
            )
        })?;
    state.remote.wake();
    Ok(Json(Envelope::ok(remote_dto(&state))))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct AccessSessionBody {
    /// 只在客户端与目标 daemon 的加密隧道内提交，不经过 hawk-server。
    access_key: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct AccessSessionDto {
    session_id: String,
    role: Role,
}

/// AccessKey rendezvous 的隧道内登录；未认证隧道只能调用此端点。
#[utoipa::path(
    post,
    path = "/api/v1/remote/access-session",
    tags = ["remote"],
    request_body = AccessSessionBody,
    responses((status = 200, description = "OK", body = Envelope<AccessSessionDto>))
)]
async fn authenticate_access_session(
    State(state): State<SharedState>,
    Extension(tunnel): Extension<crate::remote::data::TunneledPeer>,
    Json(body): Json<AccessSessionBody>,
) -> Result<Json<Envelope<AccessSessionDto>>, ApiError> {
    let Some(access) = &tunnel.access else {
        return Err(ApiError::new(
            codes::UNAUTHORIZED,
            axum::http::StatusCode::UNAUTHORIZED,
            "仅 AccessKey rendezvous 可登录",
        ));
    };
    if access.authenticated {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::CONFLICT,
            "AccessKey 会话已认证",
        ));
    }
    let credential = crate::remote::keys::sha256_hex(&body.access_key);
    let library_id = state
        .library
        .as_ref()
        .map(|lib| crate::core::paths::library_key(&lib.paths.root));
    let Some(entry) = state
        .remote
        .keys
        .resolve_for_library(library_id.as_deref(), &credential)
    else {
        return Err(ApiError::new(
            codes::UNAUTHORIZED,
            axum::http::StatusCode::UNAUTHORIZED,
            "AccessKey 无效",
        ));
    };
    state
        .remote
        .authenticate_access_session(&tunnel.session_id, credential, entry.role)
        .map_err(|status| ApiError::new(codes::UNAUTHORIZED, status, "AccessKey 会话无效"))?;
    Ok(Json(Envelope::ok(AccessSessionDto {
        session_id: tunnel.session_id,
        role: entry.role,
    })))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct AccessConnectBody {
    target_device_id: String,
    /// 配对时带外取得的目标 iroh 公钥；server 返回的其他身份一律拒绝。
    target_identity: String,
    /// 仅经本机 Loopback 代理送入目标加密隧道，不发送给 hawk-server。
    access_key: String,
}

/// AccessKey-only 原生连接：server 仅编排，目标身份钉扎后在加密隧道内登录。
#[utoipa::path(
    post,
    path = "/api/v1/remote/access-connect",
    tags = ["remote"],
    request_body = AccessConnectBody,
    responses((status = 200, description = "OK", body = Envelope<ActiveSessionDto>))
)]
async fn create_access_session(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
    Json(body): Json<AccessConnectBody>,
) -> Result<Json<Envelope<ActiveSessionDto>>, ApiError> {
    require_manage(&access)?;
    let cfg = state.remote.remote_config().current();
    if cfg.url.is_empty() {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "未配置 hawk-server 地址",
        ));
    }
    let base = cfg.url.trim_end_matches('/').to_string();
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| bad_gateway(format!("HTTP 客户端构建失败: {e}")))?;
    let identity = crate::remote::identity::Identity::load_or_create(
        state.remote.remote_config().identity_dir(),
    )
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
        .connect_access(
            &http,
            &base,
            &identity,
            &body.target_device_id,
            &body.target_identity,
            &body.access_key,
        )
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

#[derive(Serialize, utoipa::ToSchema)]
struct LeaseDto {
    expires_in: u64,
}

/// 已认证隧道续期：延长目标 daemon 的会话租约，不改变原始角色。
#[utoipa::path(
    post,
    path = "/api/v1/remote/lease",
    tags = ["remote"],
    responses((status = 200, description = "OK", body = Envelope<LeaseDto>))
)]
async fn renew_lease(
    State(state): State<SharedState>,
    Extension(tunnel): Extension<crate::remote::data::TunneledPeer>,
) -> Result<Json<Envelope<LeaseDto>>, ApiError> {
    state
        .remote
        .renew_session(&tunnel.session_id)
        .map_err(|status| ApiError::new(codes::UNAUTHORIZED, status, "会话已过期"))?;
    Ok(Json(Envelope::ok(LeaseDto { expires_in: 900 })))
}

// ---------- 设备列表与会话（admin 限定；转发自 hawk-server，key 不出本进程） ----------

/// 取接入配置并构建信令 HTTP 客户端；未配置返回 400
fn signal_client(state: &SharedState) -> Result<(reqwest::Client, String, String), ApiError> {
    let cfg = state.remote.remote_config().current();
    if cfg.url.is_empty() || cfg.tenant_key.is_empty() {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "未配置远程接入（先 PUT /api/v1/remote 设置服务端地址与服务密钥）",
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
    let session = state.remote.control_session().ok_or_else(|| {
        ApiError::new(
            codes::REMOTE_UPSTREAM,
            axum::http::StatusCode::CONFLICT,
            "设备正在接入或重新认证，请稍后重试",
        )
    })?;
    Ok((client, base, session))
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
    allow_import: bool,
    /// 是否有直连候选（UPnP 映射或全局 IPv6）；false 时直连必失败只能中继。
    /// None = 无可达性数据（未心跳过）
    #[serde(skip_serializing_if = "Option::is_none")]
    reachable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_at_ms: Option<u64>,
}

/// 从服务端设备视图的 reachability 推导直连候选存在性（与服务端候选组装同规则）
fn reachable_of(d: &Value) -> Option<bool> {
    let r = d["reachability"].as_object()?;
    Some(r["mapped_port"].is_u64() || r["public_ipv6"].as_str().is_some_and(|s| !s.is_empty()))
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
    Extension(access): Extension<AccessContext>,
) -> Result<Json<Envelope<RemoteDevicesDto>>, ApiError> {
    require_manage(&access)?;
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
                    allow_import: d["allow_import"].as_bool().unwrap_or(false),
                    reachable: reachable_of(d),
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
    Extension(access): Extension<AccessContext>,
) -> Result<Json<Envelope<Option<ActiveSessionDto>>>, ApiError> {
    require_manage(&access)?;
    Ok(Json(Envelope::ok(state.remote.sessions.snapshot())))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct CreateSessionBody {
    target_device_id: String,
    /// direct（默认，直连候选）/ relay（服务端中继，直连失败且用户确认后使用）
    #[serde(default = "default_session_mode")]
    mode: String,
}

fn default_session_mode() -> String {
    "direct".to_string()
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
    Extension(access): Extension<AccessContext>,
    Json(body): Json<CreateSessionBody>,
) -> Result<Json<Envelope<ActiveSessionDto>>, ApiError> {
    require_manage(&access)?;
    if body.mode != "direct" && body.mode != "relay" {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "mode 须为 direct 或 relay",
        ));
    }
    let (http, base, key) = signal_client(&state)?;
    let identity = crate::remote::identity::Identity::load_or_create(
        state.remote.remote_config().identity_dir(),
    )
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
    Extension(access): Extension<AccessContext>,
) -> Result<Json<super::envelope::SuccessOnly>, ApiError> {
    require_manage(&access)?;
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
    Extension(access): Extension<AccessContext>,
    Json(body): Json<PatchDeviceBody>,
) -> Result<Json<Envelope<RemoteDeviceDto>>, ApiError> {
    require_manage(&access)?;
    if body.name.is_none() && body.allow_remote.is_none() && body.library_name.is_none() {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::BAD_REQUEST,
            "至少提供一项待更新字段",
        ));
    }
    // 远程查看模式无本地库可供服务，禁止打开「允许其他设备访问」
    if body.allow_remote == Some(true) && state.library.is_none() {
        return Err(ApiError::new(
            codes::NO_LIBRARY,
            axum::http::StatusCode::BAD_REQUEST,
            "远程查看模式无本地素材库，不可被其他设备访问",
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
        allow_import: d["allow_import"].as_bool().unwrap_or(false),
        reachable: reachable_of(d),
        last_seen_at_ms: d["last_seen_at_ms"].as_u64(),
    })))
}

#[derive(Serialize, utoipa::ToSchema)]
struct RotateDeviceDto {
    /// 轮换后的新证书指纹（TOFU 锚点，旧证书会话随即失效）
    endpoint_id: String,
}

/// 注销设备记录（TenantAdmin 鉴权）：转发服务端 DELETE /devices/{id}，其进行中会话一并取消。
/// 本机当前设备不可经此移除（会删掉自己的信令身份）；先关闭远程接入是唯一安全路径
#[utoipa::path(
    delete,
    path = "/api/v1/remote/devices/{device_id}",
    tags = ["remote"],
    params(("device_id" = String, Path, description = "服务端设备 id")),
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn delete_device(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
    axum::extract::Path(device_id): axum::extract::Path<String>,
) -> Result<Json<super::envelope::SuccessOnly>, ApiError> {
    require_manage(&access)?;
    // 本机已注册身份与目标相同 → 拒绝（注销自己会让信令立即 404 重注册，记录删不干净）
    if let Ok(identity) = crate::remote::identity::Identity::load_or_create(
        state.remote.remote_config().identity_dir(),
    ) {
        if identity.device_id.as_deref() == Some(device_id.as_str()) {
            return Err(ApiError::new(
                codes::INVALID_PARAM,
                axum::http::StatusCode::BAD_REQUEST,
                "不能移除本机当前设备（如需注销请先关闭远程接入）",
            ));
        }
    }
    let cfg = state.remote.remote_config().current();
    let base = cfg.url.trim_end_matches('/').to_string();
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| bad_gateway(format!("HTTP 客户端构建失败: {e}")))?;
    let key = crate::remote::signal::tenant_session_token(&http, &base, &cfg.tenant_key)
        .await
        .map_err(bad_gateway)?;
    let resp = http
        .delete(format!("{base}/api/v1/devices/{device_id}"))
        .bearer_auth(&key)
        .send()
        .await
        .map_err(|e| bad_gateway(format!("服务端不可达: {e}")))?;
    if !resp.status().is_success() {
        return Err(bad_gateway(server_message(resp).await));
    }
    Ok(super::envelope::success())
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
    Extension(access): Extension<AccessContext>,
) -> Result<Json<Envelope<RotateDeviceDto>>, ApiError> {
    require_manage(&access)?;
    let (http, base, key) = signal_client(&state)?;
    let mut identity = crate::remote::identity::Identity::load_or_create(
        state.remote.remote_config().identity_dir(),
    )
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
    identity.rotate();
    // 先同步服务端（新身份生效），再落盘重启信令
    let device_id = identity.device_id.clone().unwrap();
    let resp = http
        .patch(format!("{base}/api/v1/devices/{device_id}"))
        .bearer_auth(&key)
        .json(&serde_json::json!({ "endpoint_id": identity.endpoint_id }))
        .send()
        .await
        .map_err(|e| bad_gateway(format!("服务端不可达: {e}")))?;
    if !resp.status().is_success() {
        return Err(bad_gateway(server_message(resp).await));
    }
    identity
        .persist(state.remote.remote_config().identity_dir())
        .map_err(|e| {
            ApiError::new(
                codes::INTERNAL,
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                e,
            )
        })?;
    state.remote.wake();
    Ok(Json(Envelope::ok(RotateDeviceDto {
        endpoint_id: identity.endpoint_id,
    })))
}

// ---------- 访问密钥（hawk-app key）管理：本地签发保管，变更后推送 server 映射 ----------

#[derive(Serialize, utoipa::ToSchema)]
struct RemoteKeyDto {
    id: String,
    label: String,
    role: Role,
    /// key 前 4 字符（识别用；明文不存也不可得）
    prefix: String,
    created_at_ms: u64,
}

fn key_dto(entry: &crate::remote::keys::KeyEntry) -> RemoteKeyDto {
    RemoteKeyDto {
        id: entry.id.clone(),
        label: entry.label.clone(),
        role: entry.role,
        prefix: entry.prefix.clone(),
        created_at_ms: entry.created_at_ms,
    }
}

#[derive(Serialize, utoipa::ToSchema)]
struct RemoteKeysDto {
    keys: Vec<RemoteKeyDto>,
}

/// 列出本机签发的访问密钥（admin 限定；不含明文——明文只在签发时返回一次）
#[utoipa::path(
    get,
    path = "/api/v1/remote/keys",
    tags = ["remote"],
    responses((status = 200, description = "OK", body = Envelope<RemoteKeysDto>))
)]
async fn list_keys(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
) -> Result<Json<Envelope<RemoteKeysDto>>, ApiError> {
    require_manage(&access)?;
    let library_id = state
        .library
        .as_ref()
        .map(|lib| crate::core::paths::library_key(&lib.paths.root));
    let keys = state
        .remote
        .keys
        .list_for_library(library_id.as_deref())
        .iter()
        .map(key_dto)
        .collect();
    Ok(Json(Envelope::ok(RemoteKeysDto { keys })))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct CreateKeyBody {
    /// 展示标签（如「手机插件」；可空）
    #[serde(default)]
    label: Option<String>,
    /// viewer 只读，importer 允许导入；自定义 importer key 至少 12 字符
    #[serde(default)]
    role: Role,
    /// 自定义密钥（可空 = 程序生成 128 位随机串）
    #[serde(default)]
    custom: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct CreateKeyDto {
    id: String,
    /// 明文 key：只在本次响应出现，本地不存明文，也不发送给 hawk-server
    key: String,
    /// 配对时必须钉扎的目标 iroh 公钥
    target_identity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_library_id: Option<String>,
}

/// 签发访问密钥（admin 限定）：本地保管验证子，只供加密隧道内的目标端认证
#[utoipa::path(
    post,
    path = "/api/v1/remote/keys",
    tags = ["remote"],
    request_body = CreateKeyBody,
    responses((status = 200, description = "OK", body = Envelope<CreateKeyDto>))
)]
async fn create_key(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
    Json(body): Json<CreateKeyBody>,
) -> Result<Json<Envelope<CreateKeyDto>>, ApiError> {
    require_manage(&access)?;
    let lib = state.require_library()?;
    let library_id = crate::core::paths::library_key(&lib.paths.root);
    let (entry, key) = state
        .remote
        .keys
        .create(
            &library_id,
            body.label.as_deref().unwrap_or(""),
            body.role,
            body.custom.as_deref(),
        )
        .map_err(|e| ApiError::new(codes::INVALID_PARAM, axum::http::StatusCode::BAD_REQUEST, e))?;
    let identity = crate::remote::identity::Identity::load_or_create(
        state.remote.remote_config().identity_dir(),
    )
    .map_err(|e| {
        ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e,
        )
    })?;
    let target_library_id = state
        .library
        .as_ref()
        .map(|lib| crate::core::paths::library_key(&lib.paths.root));
    Ok(Json(Envelope::ok(CreateKeyDto {
        id: entry.id,
        key,
        target_identity: identity.endpoint_id.clone(),
        target_device_id: identity.device_id,
        target_library_id,
    })))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct PatchKeyBody {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    role: Option<Role>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct PatchKeyDto {
    key: RemoteKeyDto,
}

/// 更新访问密钥的标签/权限（admin 限定；权限收窄对后续会话立即生效）
#[utoipa::path(
    patch,
    path = "/api/v1/remote/keys/{key_id}",
    tags = ["remote"],
    params(("key_id" = String, Path, description = "密钥 id")),
    request_body = PatchKeyBody,
    responses((status = 200, description = "OK", body = Envelope<PatchKeyDto>))
)]
async fn patch_key(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
    axum::extract::Path(key_id): axum::extract::Path<String>,
    Json(body): Json<PatchKeyBody>,
) -> Result<Json<Envelope<PatchKeyDto>>, ApiError> {
    require_manage(&access)?;
    let entry = state
        .remote
        .keys
        .patch(&key_id, body.label.as_deref(), body.role)
        .map_err(|e| ApiError::new(codes::INVALID_PARAM, axum::http::StatusCode::BAD_REQUEST, e))?
        .ok_or_else(|| {
            ApiError::new(
                codes::INVALID_PARAM,
                axum::http::StatusCode::NOT_FOUND,
                "密钥不存在",
            )
        })?;
    Ok(Json(Envelope::ok(PatchKeyDto {
        key: key_dto(&entry),
    })))
}

/// 吊销访问密钥（admin 限定）：本地删除，目标 daemon 拒绝该 key 的后续会话
#[utoipa::path(
    delete,
    path = "/api/v1/remote/keys/{key_id}",
    tags = ["remote"],
    params(("key_id" = String, Path, description = "密钥 id")),
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn delete_key(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
    axum::extract::Path(key_id): axum::extract::Path<String>,
) -> Result<Json<SuccessOnly>, ApiError> {
    require_manage(&access)?;
    let removed = state.remote.keys.remove(&key_id).map_err(|e| {
        ApiError::new(
            codes::INTERNAL,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            e,
        )
    })?;
    if !removed {
        return Err(ApiError::new(
            codes::INVALID_PARAM,
            axum::http::StatusCode::NOT_FOUND,
            "密钥不存在",
        ));
    }
    Ok(super::envelope::success())
}

/// 本机已注册的 device_id（未注册 → 400）
fn registered_device_id(state: &SharedState) -> Result<String, ApiError> {
    let identity = crate::remote::identity::Identity::load_or_create(
        state.remote.remote_config().identity_dir(),
    )
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

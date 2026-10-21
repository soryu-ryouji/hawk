//! 远程访问端点（admin 限定）：接入配置读写与运行状态。配置写回 config.toml [remote] 段，
//! 保存即热生效（remote supervisor 经 wake 热重启信令），模型与 app/lan 一致。

use super::envelope::{codes, ApiError, Envelope};
use super::{require_admin, AccessLevel, SharedState};
use crate::core::config::RemoteSettings;
use crate::remote::RemoteStatus;
use axum::extract::State;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new().routes(routes!(get_remote, put_remote))
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
    let new_remote = RemoteSettings {
        enabled: body.enabled,
        url: body.url.trim().trim_end_matches('/').to_string(),
        key: body.key.trim().to_string(),
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

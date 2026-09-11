//! lock 端点：文件夹/分类/标签锁的设置、解除与解锁（票据发放）。
//! 锁存于 .hawk/locks.toml（参与同步），密码 Argon2id 哈希；纯注册表读写（同 global_filter，
//! 不经过索引流水线），级联跟随在流水线对应 Job 内完成。变更广播 locks.changed。
//!
//! 权限：set/remove 需 admin（锁是安全边界，viewer 不可增删）；unlock 对任何有效 token
//! 开放（含只读 viewer——锁靠密码区分，不靠 token）。验证失败全局节流（连续 5 次冷却 60s）。

use crate::api::envelope::{success, ApiError, Envelope, JsonBody};
use crate::api::{AccessLevel, SharedState};
use crate::core::locks::{publish_changed, LockDim, LocksSnapshot, ManageOutcome, UnlockOutcome};
use crate::core::paths::LibraryPaths;
use crate::core::taxonomy::normalize_category_name;
use axum::extract::{Extension, State};
use axum::Json;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(lock_list))
        .routes(routes!(lock_set))
        .routes(routes!(lock_remove))
        .routes(routes!(lock_unlock))
}

/// 全部锁（仅名称，不含密码哈希；folders 为库内相对路径）
#[utoipa::path(
    get,
    path = "/api/v1/lock/list",
    tags = ["lock"],
    responses((status = 200, description = "OK", body = Envelope<LocksSnapshot>))
)]
async fn lock_list(State(state): State<SharedState>) -> Json<Envelope<LocksSnapshot>> {
    Json(Envelope::ok(state.locks.snapshot()))
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
struct LockTarget {
    /// 维度：folder / category / tag
    dimension: String,
    /// folder 为库内相对路径（如 "posters/2024"）；category/tag 为名称
    name: String,
}

/// 维度解析 + 名称校验（folder 走路径校验，category 走受控词表校验，tag 非空即可）
fn parse_target(body: &LockTarget) -> Result<(LockDim, String), ApiError> {
    let dim = LockDim::parse(&body.dimension).ok_or_else(|| {
        ApiError::invalid_param(format!("非法维度: {}（支持 folder/category/tag）", body.dimension))
    })?;
    let name = match dim {
        LockDim::Folder => {
            let n = body.name.trim();
            if n.is_empty() || !LibraryPaths::is_valid_library_path(Some(n)) {
                return Err(ApiError::invalid_param(format!("非法文件夹路径: {}", body.name)));
            }
            n.to_string()
        }
        LockDim::Category => normalize_category_name(Some(&body.name))
            .ok_or_else(|| ApiError::invalid_param(format!("非法分类名称: {}", body.name)))?,
        LockDim::Tag => {
            let n = body.name.trim();
            if n.is_empty() {
                return Err(ApiError::invalid_param("标签名称不能为空"));
            }
            n.to_string()
        }
    };
    Ok((dim, name))
}

/// 阻塞式 Argon2id 验证（约 100ms）移出异步线程
fn throttled_error(wait: std::time::Duration) -> ApiError {
    ApiError::new(
        crate::api::envelope::codes::THROTTLED,
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        format!("密码验证失败次数过多，请 {} 秒后重试", wait.as_secs().max(1)),
    )
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
struct LockSetRequest {
    #[serde(flatten)]
    target: LockTarget,
    /// 新密码（设置与修改共用）
    password: String,
    /// 已上锁条目修改密码时必须提供旧密码；首次设置省略
    #[serde(default)]
    old_password: Option<String>,
}

/// 设置/修改锁（admin 限定；已锁条目改密需旧密码）。变更经 locks.changed 广播
#[utoipa::path(
    post,
    path = "/api/v1/lock/set",
    tags = ["lock"],
    request_body = LockSetRequest,
    responses(
        (status = 200, description = "OK", body = crate::api::envelope::SuccessOnly),
        (status = 403, description = "需要 admin 权限或旧密码错误（OLD_PASSWORD_REQUIRED）")
    )
)]
async fn lock_set(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
    JsonBody(req): JsonBody<LockSetRequest>,
) -> Result<Json<crate::api::envelope::SuccessOnly>, ApiError> {
    require_admin(&access)?;
    let (dim, name) = parse_target(&req.target)?;
    if req.password.trim().is_empty() {
        return Err(ApiError::invalid_param("密码不能为空"));
    }
    let (dim_c, name_c, pwd, old) = (dim, name.clone(), req.password, req.old_password.clone());
    let locks = state.locks.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        locks.set(dim_c, &name_c, &pwd, old.as_deref())
    })
    .await
    .map_err(|e| ApiError::internal(format!("密码哈希任务失败: {e}")))?;
    match outcome {
        ManageOutcome::Ok => {
            publish_changed(&state.bus, &state.locks.snapshot());
            Ok(success())
        }
        ManageOutcome::WrongOldPassword => Err(ApiError::new(
            crate::api::envelope::codes::OLD_PASSWORD_REQUIRED,
            axum::http::StatusCode::FORBIDDEN,
            "该条目已上锁，修改密码需要提供正确的旧密码",
        )),
        ManageOutcome::NotFound => unreachable!("set 不返回 NotFound"),
        ManageOutcome::Throttled(wait) => Err(throttled_error(wait)),
    }
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
struct LockRemoveRequest {
    #[serde(flatten)]
    target: LockTarget,
    password: String,
}

/// 解除锁（admin 限定；需密码）。变更经 locks.changed 广播
#[utoipa::path(
    post,
    path = "/api/v1/lock/remove",
    tags = ["lock"],
    request_body = LockRemoveRequest,
    responses((status = 200, description = "OK", body = crate::api::envelope::SuccessOnly))
)]
async fn lock_remove(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessLevel>,
    JsonBody(req): JsonBody<LockRemoveRequest>,
) -> Result<Json<crate::api::envelope::SuccessOnly>, ApiError> {
    require_admin(&access)?;
    let (dim, name) = parse_target(&req.target)?;
    let (dim_c, name_c, pwd) = (dim, name, req.password);
    let locks = state.locks.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        locks.remove(dim_c, &name_c, &pwd)
    })
    .await
    .map_err(|e| ApiError::internal(format!("密码验证任务失败: {e}")))?;
    match outcome {
        ManageOutcome::Ok => {
            publish_changed(&state.bus, &state.locks.snapshot());
            Ok(success())
        }
        ManageOutcome::NotFound => Err(ApiError::lock_not_found(&req.target.name)),
        ManageOutcome::WrongOldPassword => Err(ApiError::unauthorized("密码错误")),
        ManageOutcome::Throttled(wait) => Err(throttled_error(wait)),
    }
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
struct LockUnlockRequest {
    #[serde(flatten)]
    target: LockTarget,
    password: String,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
struct UnlockResponse {
    /// 解锁票据：后续请求经 `X-Hawk-Unlock` 头或 `?unlock=` 查询参数附带（img/SSE 通道）；
    /// daemon 重启失效，前端丢弃即「锁定回去」。注意：不要把带 unlock 参数的图片 URL 分享给他人
    unlock_token: String,
}

/// 解锁（任何有效 token，含只读 viewer）：校验密码发放票据，票据由各客户端独立持有
#[utoipa::path(
    post,
    path = "/api/v1/lock/unlock",
    tags = ["lock"],
    request_body = LockUnlockRequest,
    responses(
        (status = 200, description = "OK", body = Envelope<UnlockResponse>),
        (status = 401, description = "密码错误或锁不存在"),
        (status = 429, description = "失败次数过多，冷却中")
    )
)]
async fn lock_unlock(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<LockUnlockRequest>,
) -> Result<Json<Envelope<UnlockResponse>>, ApiError> {
    let (dim, name) = parse_target(&req.target)?;
    let (dim_c, name_c, pwd) = (dim, name, req.password);
    let locks = state.locks.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        locks.unlock(dim_c, &name_c, &pwd)
    })
    .await
    .map_err(|e| ApiError::internal(format!("密码验证任务失败: {e}")))?;
    match outcome {
        UnlockOutcome::Granted(ticket) => Ok(Json(Envelope::ok(UnlockResponse {
            unlock_token: ticket,
        }))),
        UnlockOutcome::NotFound => Err(ApiError::lock_not_found(&req.target.name)),
        UnlockOutcome::WrongPassword => Err(ApiError::unauthorized("密码错误")),
        UnlockOutcome::Throttled(wait) => Err(throttled_error(wait)),
    }
}

fn require_admin(access: &AccessLevel) -> Result<(), ApiError> {
    if matches!(access, AccessLevel::Admin) {
        Ok(())
    } else {
        Err(ApiError::new(
            crate::api::envelope::codes::READ_ONLY,
            axum::http::StatusCode::FORBIDDEN,
            "viewer token cannot manage locks",
        ))
    }
}

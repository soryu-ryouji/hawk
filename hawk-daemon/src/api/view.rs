//! view 端点：视图偏好（排序记忆）。条目以 scope 键扁平存储于 .hawk/view.toml（参与同步）。
//! 偏好与索引/元数据无耦合，注册表自带锁，端点直接读写（不经过索引流水线）。

use crate::api::envelope::{success, ApiError, Envelope, JsonBody, SuccessOnly};
use crate::api::SharedState;
use crate::core::view_prefs::{try_normalize_sort, try_parse_scope, ViewSort};
use axum::extract::{Query, State};
use axum::Json;
use serde::Serialize;
use std::collections::HashMap;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(preferences))
        .routes(routes!(preference_put))
        .routes(routes!(preference_delete))
}

#[derive(Serialize, utoipa::ToSchema)]
struct ViewSortDto {
    order_by: String,
    order: String,
}

impl From<&ViewSort> for ViewSortDto {
    fn from(sort: &ViewSort) -> Self {
        ViewSortDto {
            order_by: sort.order_by.clone(),
            order: sort.order.clone(),
        }
    }
}

/// 全部视图偏好（scope → 排序）
#[utoipa::path(
    get,
    path = "/api/v1/view/preferences",
    tags = ["view"],
    responses((status = 200, description = "OK", body = Envelope<HashMap<String, ViewSortDto>>))
)]
async fn preferences(State(state): State<SharedState>) -> Json<Envelope<HashMap<String, ViewSortDto>>> {
    let snapshot = state.prefs.snapshot();
    let dto: HashMap<String, ViewSortDto> = snapshot
        .iter()
        .map(|(k, v)| (k.clone(), ViewSortDto::from(v)))
        .collect();
    Json(Envelope::ok(dto))
}

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
struct ViewPreferencePutRequest {
    scope: String,
    order_by: String,
    order: String,
}

/// 写一条视图偏好（覆盖写；scope 与排序值经校验规范化）
#[utoipa::path(
    put,
    path = "/api/v1/view/preference",
    tags = ["view"],
    request_body = ViewPreferencePutRequest,
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn preference_put(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ViewPreferencePutRequest>,
) -> Result<Json<SuccessOnly>, ApiError> {
    let scope = try_parse_scope(&req.scope)
        .ok_or_else(|| ApiError::invalid_param(format!("非法作用域: {}", req.scope)))?;
    let sort = try_normalize_sort(&req.order_by, &req.order)
        .ok_or_else(|| ApiError::invalid_param(format!("非法排序值: {}/{}", req.order_by, req.order)))?;
    state.prefs.set(&scope, sort);
    Ok(success())
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ScopeQuery {
    /// 视图作用域：folder:<路径>（"" 为库根）/category:<名>/tag:<名>
    scope: String,
}

/// 删除一条视图偏好（不存在则无动作）
#[utoipa::path(
    delete,
    path = "/api/v1/view/preference",
    tags = ["view"],
    params(ScopeQuery),
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn preference_delete(
    State(state): State<SharedState>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<SuccessOnly>, ApiError> {
    let scope = try_parse_scope(&q.scope)
        .ok_or_else(|| ApiError::invalid_param(format!("非法作用域: {}", q.scope)))?;
    state.prefs.delete(&scope);
    Ok(success())
}

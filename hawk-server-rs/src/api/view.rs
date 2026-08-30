//! view 端点：视图偏好（排序记忆）。条目以 scope 键扁平存储于 .hawk/view.toml（参与同步）。
//! 偏好与索引/元数据无耦合，注册表自带锁，端点直接读写（不经过索引流水线）。

use crate::api::envelope::{success, ApiError, Envelope, JsonBody};
use crate::api::SharedState;
use crate::core::view_prefs::{try_normalize_sort, try_parse_scope, ViewSort};
use axum::extract::{Query, State};
use axum::routing::{get, delete, put};
use axum::{Json, Router};
use serde::Serialize;
use std::collections::HashMap;

pub fn routes() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/view/preferences", get(preferences))
        .route("/api/v1/view/preference", put(preference_put))
        .route("/api/v1/view/preference", delete(preference_delete))
}

#[derive(Serialize)]
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

async fn preferences(State(state): State<SharedState>) -> Json<Envelope<HashMap<String, ViewSortDto>>> {
    let snapshot = state.prefs.snapshot();
    let dto: HashMap<String, ViewSortDto> = snapshot
        .iter()
        .map(|(k, v)| (k.clone(), ViewSortDto::from(v)))
        .collect();
    Json(Envelope::ok(dto))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct ViewPreferencePutRequest {
    scope: String,
    order_by: String,
    order: String,
}

async fn preference_put(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ViewPreferencePutRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scope = try_parse_scope(&req.scope)
        .ok_or_else(|| ApiError::invalid_param(format!("非法作用域: {}", req.scope)))?;
    let sort = try_normalize_sort(&req.order_by, &req.order)
        .ok_or_else(|| ApiError::invalid_param(format!("非法排序值: {}/{}", req.order_by, req.order)))?;
    state.prefs.set(&scope, sort);
    Ok(Json(success()))
}

#[derive(serde::Deserialize)]
struct ScopeQuery {
    scope: String,
}

async fn preference_delete(
    State(state): State<SharedState>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scope = try_parse_scope(&q.scope)
        .ok_or_else(|| ApiError::invalid_param(format!("非法作用域: {}", q.scope)))?;
    state.prefs.delete(&scope);
    Ok(Json(success()))
}

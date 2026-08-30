//! category / tag 端点。分类与标签同为扁平名字（item 可同时挂多个）；
//! 区别：标签完全自由，分类需先创建（可空挂），用于受控词表。
//! 注册表与元数据批量迁移均由索引流水线执行（单写者）；校验发生在端点层。

use crate::api::envelope::{success, ApiError, Envelope, JsonBody};
use crate::api::SharedState;
use crate::core::taxonomy::normalize_category_name;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

pub fn routes() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/category/list", get(category_list))
        .route("/api/v1/category/create", post(category_create))
        .route("/api/v1/category/update", post(category_update))
        .route("/api/v1/category/delete", post(category_delete))
        .route("/api/v1/tag/list", get(tag_list))
        .route("/api/v1/tag/create", post(tag_create))
        .route("/api/v1/tag/update", post(tag_update))
        .route("/api/v1/tag/delete", post(tag_delete))
}

#[derive(Serialize)]
struct TaxonInfo {
    name: String,
    count: usize,
}

#[derive(serde::Deserialize)]
struct CategoryCreateRequest {
    name: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
struct CategoryUpdateRequest {
    name: String,
    new_name: String,
}

#[derive(serde::Deserialize)]
struct CategoryNameRequest {
    name: String,
}

/// 分类列表 = 注册表 ∪ 全部 item 赋值并集；count 为库内（不含回收站）item 数
async fn category_list(State(state): State<SharedState>) -> Json<Envelope<Vec<TaxonInfo>>> {
    let counts = state.index.category_counts();
    let mut names: Vec<String> = state.categories.snapshot();
    for n in counts.keys() {
        if !names.contains(n) {
            names.push(n.clone());
        }
    }
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    Json(Envelope::ok(
        names
            .into_iter()
            .map(|n| TaxonInfo {
                count: counts.get(&n).copied().unwrap_or(0),
                name: n,
            })
            .collect(),
    ))
}

async fn category_create(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<CategoryCreateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = normalize_category_name(Some(&req.name))
        .ok_or_else(|| ApiError::invalid_param(format!("非法分类名称: {}", req.name)))?;
    if category_exists(&state, &name) {
        return Err(ApiError::category_exists(&name));
    }
    state.pipeline.submit_category_create(name).await.map_err(ApiError::internal)?;
    Ok(Json(success()))
}

async fn category_update(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<CategoryUpdateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = normalize_category_name(Some(&req.name))
        .ok_or_else(|| ApiError::invalid_param(format!("非法分类名称: {}", req.name)))?;
    if !category_exists(&state, &name) {
        return Err(ApiError::category_not_found(&name));
    }
    let new_name = normalize_category_name(Some(&req.new_name))
        .ok_or_else(|| ApiError::invalid_param(format!("非法分类名称: {}", req.new_name)))?;
    if new_name != name {
        state
            .pipeline
            .submit_category_update(name, new_name)
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(Json(success()))
}

async fn category_delete(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<CategoryNameRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = normalize_category_name(Some(&req.name))
        .ok_or_else(|| ApiError::invalid_param(format!("非法分类名称: {}", req.name)))?;
    if !category_exists(&state, &name) {
        return Err(ApiError::category_not_found(&name));
    }
    state.pipeline.submit_category_delete(name).await.map_err(ApiError::internal)?;
    Ok(Json(success()))
}

/// 标签列表 = 注册表 ∪ 赋值并集；count 为库内（不含回收站）item 数
async fn tag_list(State(state): State<SharedState>) -> Json<Envelope<Vec<TaxonInfo>>> {
    let counts: std::collections::HashMap<String, usize> =
        state.index.tags_with_counts().into_iter().collect();
    let mut names: Vec<String> = state.tags.snapshot();
    for n in counts.keys() {
        if !names.contains(n) {
            names.push(n.clone());
        }
    }
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    Json(Envelope::ok(
        names
            .into_iter()
            .map(|n| TaxonInfo {
                count: counts.get(&n).copied().unwrap_or(0),
                name: n,
            })
            .collect(),
    ))
}

async fn tag_create(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<CategoryCreateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = normalize_tag(&req.name)?;
    state.pipeline.submit_tag_create(name).await.map_err(ApiError::internal)?;
    Ok(Json(success()))
}

async fn tag_update(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<CategoryUpdateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = normalize_tag(&req.name)?;
    if !tag_exists(&state, &name) {
        return Err(ApiError::tag_not_found(&name));
    }
    let new_name = normalize_tag(&req.new_name)?;
    if new_name != name {
        state
            .pipeline
            .submit_tag_update(name, new_name)
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(Json(success()))
}

async fn tag_delete(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<CategoryNameRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let name = normalize_tag(&req.name)?;
    if !tag_exists(&state, &name) {
        return Err(ApiError::tag_not_found(&name));
    }
    state.pipeline.submit_tag_delete(name).await.map_err(ApiError::internal)?;
    Ok(Json(success()))
}

/// 分类存在性：注册表 ∪ 全部 item 赋值（含回收站）
fn category_exists(state: &SharedState, name: &str) -> bool {
    state.categories.contains(name) || state.index.all_categories().iter().any(|c| c == name)
}

fn tag_exists(state: &SharedState, name: &str) -> bool {
    state.tags.contains(name)
        || state
            .index
            .tags_with_counts()
            .iter()
            .any(|(n, _)| n == name)
}

fn normalize_tag(name: &str) -> Result<String, ApiError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ApiError::invalid_param("标签名称不能为空"));
    }
    Ok(trimmed.to_string())
}

//! library 端点：info / reindex / rescan / refresh_cache

use crate::api::envelope::{success, ApiError, Envelope, JsonBody, SuccessOnly};
use crate::api::SharedState;
use crate::core::index::RefreshScope;
use crate::core::paths::unix_ms;
use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(library_info))
        .routes(routes!(reindex))
        .routes(routes!(rescan))
        .routes(routes!(refresh_cache))
}

#[derive(Serialize, utoipa::ToSchema)]
struct LibraryInfo {
    name: String,
    path: String,
    modification_time: i64,
    application_version: &'static str,
}

/// 库信息：显示名取 config 的 name，缺省目录名
#[utoipa::path(
    get,
    path = "/api/v1/library/info",
    tags = ["library"],
    responses((status = 200, description = "OK", body = Envelope<LibraryInfo>))
)]
async fn library_info(State(state): State<SharedState>) -> Json<Envelope<LibraryInfo>> {
    let root = &state.paths.root;
    let name = state
        .config
        .current()
        .name
        .clone()
        .unwrap_or_else(|| root.trim_end_matches('/').rsplit('/').next().unwrap_or(root).to_string());
    let modification_time = std::fs::metadata(root)
        .and_then(|m| m.modified())
        .map(unix_ms)
        .unwrap_or(0);
    Json(Envelope::ok(LibraryInfo {
        name,
        path: root.clone(),
        modification_time,
        application_version: env!("CARGO_PKG_VERSION"),
    }))
}

/// 全量重建索引：重算全部哈希，异步执行，立即返回
#[utoipa::path(
    post,
    path = "/api/v1/library/reindex",
    tags = ["library"],
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn reindex(State(state): State<SharedState>) -> Json<SuccessOnly> {
    state.pipeline.request_scan(true);
    success()
}

/// 刷新缓存：忽略快照强制遍历全部文件做复用判定（不读文件内容）。异步执行，立即返回
#[utoipa::path(
    post,
    path = "/api/v1/library/rescan",
    tags = ["library"],
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
async fn rescan(State(state): State<SharedState>) -> Json<SuccessOnly> {
    state.pipeline.request_rescan();
    success()
}

#[derive(Deserialize, utoipa::ToSchema)]
struct RefreshCacheRequest {
    /// folder | category | tag | library
    #[serde(rename = "type")]
    scope_type: String,
    /// folder/category/tag 的名称（folder 为目录相对路径，空串 = 库根）；library 时忽略
    value: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
struct RefreshCacheResponse {
    /// 实际入队的修复任务数（in-flight 去重丢弃或源文件不在的不计）
    dispatched: usize,
}

/// 按范围刷新派生缓存（补缺失模式）：对范围内全部 item 派发修复任务——
/// 补缺失宽高（0 × 0）+ 生成缺失尺寸缩略图 + 提炼缺失调色板，不重建已有文件。
/// 用户遇到显示异常时的手动修复入口；异步执行立即返回，积压经 task.progress(thumbnail) 可见
#[utoipa::path(
    post,
    path = "/api/v1/library/refresh_cache",
    tags = ["library"],
    request_body = RefreshCacheRequest,
    responses((status = 200, description = "OK", body = Envelope<RefreshCacheResponse>))
)]
async fn refresh_cache(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<RefreshCacheRequest>,
) -> Result<Json<Envelope<RefreshCacheResponse>>, ApiError> {
    let scope = match req.scope_type.as_str() {
        "library" => RefreshScope::Library,
        "folder" => RefreshScope::Folder(req.value.unwrap_or_default()),
        "category" => match req.value.filter(|v| !v.is_empty()) {
            Some(v) => RefreshScope::Category(v),
            None => return Err(ApiError::invalid_param("category 需提供 value")),
        },
        "tag" => match req.value.filter(|v| !v.is_empty()) {
            Some(v) => RefreshScope::Tag(v),
            None => return Err(ApiError::invalid_param("tag 需提供 value")),
        },
        other => return Err(ApiError::invalid_param(format!("未知范围类型: {other}"))),
    };
    let hashes = state.index.hashes_in_scope(&scope);
    let mut dispatched = 0;
    for hash in &hashes {
        if let Some(abs) = state.index.main_source_abs(hash, &state.paths) {
            if state.worker.enqueue_thumbs(hash, &abs) {
                dispatched += 1;
            }
        }
    }
    Ok(Json(Envelope::ok(RefreshCacheResponse { dispatched })))
}

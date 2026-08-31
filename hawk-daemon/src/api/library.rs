//! library 端点：info / reindex / rescan / refresh_cache

use crate::api::envelope::{success, ApiError, Envelope};
use crate::api::SharedState;
use crate::core::index::RefreshScope;
use crate::core::paths::unix_ms;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::envelope::JsonBody;

pub fn routes() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/library/info", get(library_info))
        .route("/api/v1/library/reindex", post(reindex))
        .route("/api/v1/library/rescan", post(rescan))
        .route("/api/v1/library/refresh_cache", post(refresh_cache))
}

#[derive(Serialize)]
struct LibraryInfo {
    name: String,
    path: String,
    modification_time: i64,
    application_version: &'static str,
    thumbnail_sizes: Vec<i32>,
}

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
        thumbnail_sizes: state.config.current().thumbnail_sizes,
    }))
}

/// 全量重建索引：重算全部哈希，异步执行，立即返回
async fn reindex(State(state): State<SharedState>) -> Json<serde_json::Value> {
    state.pipeline.request_scan(true);
    Json(success())
}

/// 刷新缓存：忽略快照强制遍历全部文件做复用判定（不读文件内容）。异步执行，立即返回
async fn rescan(State(state): State<SharedState>) -> Json<serde_json::Value> {
    state.pipeline.request_rescan();
    Json(success())
}

#[derive(Deserialize)]
struct RefreshCacheRequest {
    /// folder | category | tag | library
    #[serde(rename = "type")]
    scope_type: String,
    /// folder/category/tag 的名称（folder 为目录相对路径，空串 = 库根）；library 时忽略
    value: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct RefreshCacheResponse {
    /// 实际入队的修复任务数（in-flight 去重丢弃或源文件不在的不计）
    dispatched: usize,
}

/// 按范围刷新派生缓存（补缺失模式）：对范围内全部 item 派发修复任务——
/// 补缺失宽高（0 × 0）+ 生成缺失尺寸缩略图 + 提炼缺失调色板，不重建已有文件。
/// 用户遇到显示异常时的手动修复入口；异步执行立即返回，积压经 task.progress(thumbnail) 可见
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

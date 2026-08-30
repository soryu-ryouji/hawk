//! library 端点：info / reindex / rescan

use crate::api::envelope::{success, Envelope};
use crate::api::SharedState;
use crate::core::paths::unix_ms;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

pub fn routes() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/library/info", get(library_info))
        .route("/api/v1/library/reindex", post(reindex))
        .route("/api/v1/library/rescan", post(rescan))
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

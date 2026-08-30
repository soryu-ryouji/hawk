//! trash 端点：清空回收站。
//! 顺序铁律：先由流水线清理索引位置、元数据与缓存（缩略图/调色板），再物理删除。
//! 先物理删除会让 watcher 的 Deleted 事件抢先摘除位置，导致元数据与缓存泄漏

use crate::api::envelope::{success, ApiError};
use crate::api::SharedState;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};

pub fn routes() -> Router<SharedState> {
    Router::new().route("/api/v1/trash/clear", post(trash_clear))
}

async fn trash_clear(State(state): State<SharedState>) -> Result<Json<serde_json::Value>, ApiError> {
    state.pipeline.submit_clear_trash().await.map_err(ApiError::internal)?;

    let entries = match std::fs::read_dir(&state.paths.trash_dir) {
        Ok(e) => e,
        Err(e) => return Err(ApiError::internal(format!("读取回收站失败: {e}"))),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let result = if is_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = result {
            tracing::warn!("删除回收站内容失败 {}: {e}", path.display());
        }
    }
    Ok(Json(success()))
}

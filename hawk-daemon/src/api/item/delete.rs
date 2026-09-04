//! item/delete 与 restore：回收站进出。

use super::*;

// ---------- delete / restore ----------

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemIdRequest {
    id: String,
    path: Option<String>,
}

pub(crate) async fn item_delete(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemIdRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // 不带 path = 卡片级删除：回收全部库内位置。同内容多路径 item（重复导入同名/异名文件）
    // 只回收一个位置会让卡片残留在网格，用户感知为「删除不生效」；带 path = 单位置删除
    let locs = if let Some(path) = req.path.as_deref() {
        vec![state
            .index
            .find_location(&req.id, Some(path), Some(false))
            .ok_or_else(|| ApiError::invalid_param(format!("库内不存在该文件位置: {path}")))?]
    } else {
        let all = state.index.item_locations(&req.id, Some(false));
        if all.is_empty() {
            return Err(ApiError::invalid_param("item 不在库内"));
        }
        all
    };

    for loc in locs {
        let source_abs = state.paths.to_absolute(&loc.path).unwrap();
        let trash_abs = fs_util::find_free_trash_path(&state.paths, &loc.path, false);
        fs_util::ensure_parent_dir(&trash_abs);
        std::fs::rename(&source_abs, &trash_abs).map_err(|e| ApiError::internal(format!("移入回收站失败: {e}")))?;
        state.pipeline.submit_move(source_abs, trash_abs).await.map_err(ApiError::internal)?;
    }
    Ok(Json(success()))
}

pub(crate) async fn item_restore(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemIdRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // 与 delete 对称：不带 path 恢复全部回收站位置；同名冲突的位置跳过留在回收站，
    // 全部冲突才报 FILE_EXISTS（部分恢复不整体回滚）
    let locs = if let Some(path) = req.path.as_deref() {
        vec![state
            .index
            .find_location(&req.id, Some(path), Some(true))
            .ok_or_else(|| ApiError::invalid_param(format!("回收站中不存在该文件位置: {path}")))?]
    } else {
        let all = state.index.item_locations(&req.id, Some(true));
        if all.is_empty() {
            return Err(ApiError::invalid_param("item 不在回收站"));
        }
        all
    };

    let mut restored = 0;
    let mut first_conflict: Option<String> = None;
    for loc in locs {
        // 按原路径放回(回收站中的实际名称去掉 .hawk/trash/ 前缀)
        let target_abs = state.paths.to_absolute(&loc.library_path).unwrap();
        if std::path::Path::new(&target_abs).exists() {
            first_conflict.get_or_insert_with(|| loc.library_path.clone());
            continue;
        }
        let source_abs = state.paths.to_absolute(&loc.path).unwrap();
        fs_util::ensure_parent_dir(&target_abs);
        std::fs::rename(&source_abs, &target_abs).map_err(|e| ApiError::internal(format!("恢复失败: {e}")))?;
        state.pipeline.submit_move(source_abs, target_abs).await.map_err(ApiError::internal)?;
        restored += 1;
    }
    if restored == 0 {
        return Err(ApiError::file_exists(first_conflict.unwrap_or_default()));
    }
    Ok(Json(success()))
}

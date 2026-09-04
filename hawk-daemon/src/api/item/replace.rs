//! item/replace：内容替换（客户端编辑后的回写；校验、哈希、写盘、id 漂移闭环）。

use super::*;

// ---------- replace ----------

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemReplaceRequest {
    id: String,
    path: Option<String>,
    img_base64: String,
}

/// 内容替换(item/replace):客户端编辑(旋转/裁切等)后的新内容提交存储层。
/// 哈希变化 → id 漂移,元数据继承迁移/事件/缩略图重建由索引流水线闭环。
/// 写回时保留原文件的修改时间（修正性编辑不改变素材的时序位置）
pub(crate) async fn item_replace(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemReplaceRequest>,
) -> Result<Json<Envelope<ItemDto>>, ApiError> {
    let loc = find_location(&state, &req.id, req.path.as_deref(), None)?;
    if loc.in_trash {
        return Err(ApiError::invalid_param("回收站中的文件不支持内容替换,请先恢复"));
    }

    let bytes = decode_base64(&req.img_base64)?;

    // 内容必须是图像且格式与文件扩展名一致:扩展名与内容错位会破坏类型推断与预览
    let file_ext = LibraryPaths::ext_of(&loc.library_path);
    let ext = ThumbnailService::detect_extension_bytes(&bytes)
        .ok_or_else(|| ApiError::unsupported_format("无法识别的图像数据"))?;
    if ext != file_ext {
        return Err(ApiError::unsupported_format(format!(
            "图像格式({ext})与文件扩展名({file_ext})不一致"
        )));
    }

    let hash = content_hash::hash_bytes(&bytes);
    if hash == req.id {
        // 内容未变化(幂等):不触发漂移,直接返回当前投影
        let dto = state.index.get_dto(&req.id).ok_or_else(|| ApiError::item_not_found(&req.id))?;
        return Ok(Json(Envelope::ok(dto)));
    }

    let target_abs = state.paths.to_absolute(&loc.path).unwrap();
    // 写回（阻塞 IO 移出运行时线程）保留原文件的修改时间（修正性编辑不改变素材的时序位置）
    tokio::task::spawn_blocking({
        let target = target_abs.clone();
        let data = bytes.clone();
        move || {
            let mtime = std::fs::metadata(&target).ok().and_then(|m| m.modified().ok());
            std::fs::write(&target, &data).map_err(|e| format!("写回文件失败: {e}"))?;
            if let Some(mtime) = mtime {
                let _ = filetime::set_file_mtime(&target, filetime::FileTime::from_system_time(mtime));
            }
            Ok::<(), String>(())
        }
    })
    .await
    .map_err(|e| ApiError::internal(format!("写回任务失败: {e}")))?
    .map_err(ApiError::internal)?;

    let result = state
        .pipeline
        .submit_upsert(target_abs, Some(hash))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("索引失败"))?;
    Ok(Json(Envelope::ok(result.item)))
}

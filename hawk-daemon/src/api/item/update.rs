//! item/update 与 batch_update：元数据修改（批量走并集追加语义）。

use super::*;

// ---------- update ----------

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemUpdateRequest {
    id: String,
    path: Option<String>,
    name: Option<String>,
    tags: Option<Vec<String>>,
    folder_path: Option<String>,
    star: Option<i32>,
    categories: Option<Vec<String>>,
    annotation: Option<String>,
    url: Option<String>,
}

/// 元数据修改：改名/移动做真实文件操作后经流水线同步；tags/categories/star/annotation/url 直接写元数据
#[utoipa::path(
    post,
    path = "/api/v1/item/update",
    tags = ["item"],
    request_body = ItemUpdateRequest,
    responses((status = 200, description = "OK", body = Envelope<ItemDto>))
)]
pub(crate) async fn item_update(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemUpdateRequest>,
) -> Result<Json<Envelope<ItemDto>>, ApiError> {
    let loc = find_location(&state, &req.id, req.path.as_deref(), None)?;

    if loc.in_trash && (req.name.is_some() || req.folder_path.is_some()) {
        return Err(ApiError::invalid_param("回收站中的文件不支持改名/移动,请先恢复"));
    }

    if let Some(name) = &req.name {
        if !fs_util::is_valid_name(Some(name)) {
            return Err(ApiError::invalid_param(format!("非法文件名: {name}")));
        }
        let ext = LibraryPaths::ext_of(&loc.library_path);
        let file_name = if ext.is_empty() { name.clone() } else { format!("{name}.{ext}") };
        let dir = LibraryPaths::dir_of(&loc.path);
        let target_rel = if dir.is_empty() { file_name.clone() } else { format!("{dir}/{file_name}") };
        if target_rel != loc.path {
            let source_abs = state.paths.to_absolute(&loc.path).unwrap();
            let target_abs = state.paths.to_absolute(&target_rel).unwrap();
            if std::path::Path::new(&target_abs).exists() {
                return Err(ApiError::file_exists(&target_rel));
            }
            std::fs::rename(&source_abs, &target_abs).map_err(|e| ApiError::internal(format!("重命名失败: {e}")))?;
            state.pipeline.submit_move(source_abs, target_abs).await.map_err(ApiError::internal)?;
        }
    }

    if let Some(folder_path) = &req.folder_path {
        let folder_abs = if folder_path.is_empty() {
            state.paths.root.clone()
        } else {
            if !LibraryPaths::is_valid_library_path(Some(folder_path)) {
                return Err(ApiError::invalid_param(format!("非法文件夹路径: {folder_path}")));
            }
            let abs = state.paths.to_absolute(folder_path).unwrap();
            if !std::path::Path::new(&abs).is_dir() {
                return Err(ApiError::folder_not_found(folder_path));
            }
            abs
        };
        // name 分支可能已移动过文件:按移动后的最新位置再移动(改名+移动同请求时基于新文件名计算目标)
        let current = find_location(&state, &req.id, req.path.as_deref(), None)?;
        let file_name = current.path.rsplit('/').next().unwrap_or(&current.path).to_string();
        let target_abs = format!("{folder_abs}/{file_name}");
        let source_abs = state.paths.to_absolute(&current.path).unwrap();
        if target_abs != source_abs {
            if std::path::Path::new(&target_abs).exists() {
                let rel = if folder_path.is_empty() {
                    file_name.clone()
                } else {
                    format!("{folder_path}/{file_name}")
                };
                return Err(ApiError::file_exists(rel));
            }
            std::fs::rename(&source_abs, &target_abs).map_err(|e| ApiError::internal(format!("移动失败: {e}")))?;
            state.pipeline.submit_move(source_abs, target_abs).await.map_err(ApiError::internal)?;
        }
    }

    if let Some(star) = req.star {
        if !(0..=5).contains(&star) {
            return Err(ApiError::invalid_param("star 取值范围为 0-5"));
        }
    }

    if req.tags.is_some() || req.star.is_some() || req.categories.is_some() || req.annotation.is_some() || req.url.is_some() {
        let categories = normalize_categories(req.categories.as_deref())?;
        let tags = req.tags.clone();
        let star = req.star;
        let annotation = req.annotation.clone();
        let url = req.url.clone();
        state
            .pipeline
            .submit_metadata(req.id.clone(), move |meta| {
                if let Some(tags) = tags {
                    meta.tags = tags;
                }
                if let Some(categories) = categories {
                    meta.categories = categories;
                }
                if let Some(star) = star {
                    meta.star = star;
                }
                if let Some(annotation) = annotation {
                    meta.annotation = Some(annotation);
                }
                if let Some(url) = url {
                    meta.url = Some(url);
                }
            })
            .await
            .map_err(ApiError::internal)?;
    }

    let dto = state.index.get_dto(&req.id).ok_or_else(|| ApiError::item_not_found(&req.id))?;
    Ok(Json(Envelope::ok(dto)))
}

// ---------- batch_update ----------

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemBatchUpdateRequest {
    ids: Vec<String>,
    /// 与 ids 等长的可选位置限定（同内容多位置时按位置移动 folder_path；缺省或元素为 null 时取主位置）
    paths: Option<Vec<Option<String>>>,
    add_tags: Option<Vec<String>>,
    add_categories: Option<Vec<String>>,
    star: Option<i32>,
    folder_path: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemBatchUpdateResponse {
    updated: usize,
    missing_ids: Vec<String>,
}

/// 批量更新：标签/分类并集追加，评分/文件夹设置；不存在的 id 记入 missing_ids 不整体失败
#[utoipa::path(
    post,
    path = "/api/v1/item/batch_update",
    tags = ["item"],
    request_body = ItemBatchUpdateRequest,
    responses((status = 200, description = "OK", body = Envelope<ItemBatchUpdateResponse>))
)]
pub(crate) async fn item_batch_update(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemBatchUpdateRequest>,
) -> Result<Json<Envelope<ItemBatchUpdateResponse>>, ApiError> {
    if req.ids.is_empty() {
        return Err(ApiError::invalid_param("ids 不能为空"));
    }
    if let Some(paths) = &req.paths {
        if paths.len() != req.ids.len() {
            return Err(ApiError::invalid_param("paths 须与 ids 等长"));
        }
    }
    if req.add_tags.is_none() && req.add_categories.is_none() && req.star.is_none() && req.folder_path.is_none() {
        return Err(ApiError::invalid_param("至少提供一个更新字段"));
    }
    if let Some(star) = req.star {
        if !(0..=5).contains(&star) {
            return Err(ApiError::invalid_param("star 取值范围为 0-5"));
        }
    }
    let add_categories = normalize_categories(req.add_categories.as_deref())?;

    let mut ids = req.ids.clone();
    ids.dedup();
    let mut move_failed: Vec<String> = Vec::new();

    // folder_path:逐个移动指定位置(库内;缺省主位置);已在目标处的跳过;无库内位置(全在回收站)的移动不适用,跳过
    if let Some(folder_path) = &req.folder_path {
        let folder_abs = if folder_path.is_empty() {
            state.paths.root.clone()
        } else {
            if !LibraryPaths::is_valid_library_path(Some(folder_path)) {
                return Err(ApiError::invalid_param(format!("非法文件夹路径: {folder_path}")));
            }
            let abs = state.paths.to_absolute(folder_path).unwrap();
            if !std::path::Path::new(&abs).is_dir() {
                return Err(ApiError::folder_not_found(folder_path));
            }
            abs
        };
        for (i, id) in ids.iter().enumerate() {
            let path = req.paths.as_ref().and_then(|ps| ps.get(i)).and_then(|p| p.as_deref());
            let Some(loc) = state.index.find_location(id, path, Some(false)) else {
                continue;
            };
            let file_name = loc.path.rsplit('/').next().unwrap_or(&loc.path).to_string();
            let source_abs = state.paths.to_absolute(&loc.path).unwrap();
            let target_abs = format!("{folder_abs}/{file_name}");
            if target_abs == source_abs {
                continue;
            }
            // 同名冲突不整体失败:跳过该项移动并记入 missing,其余照常
            if std::path::Path::new(&target_abs).exists() {
                move_failed.push(id.clone());
                continue;
            }
            if std::fs::rename(&source_abs, &target_abs).is_err() {
                move_failed.push(id.clone());
                continue;
            }
            state.pipeline.submit_move(source_abs, target_abs).await.map_err(ApiError::internal)?;
        }
    }

    // 元数据:标签/分类并集追加、评分设置;一次提交,由流水线批量应用(单写者)
    let mut updated = 0;
    let mut missing: Vec<String> = move_failed;
    if req.add_tags.is_some() || add_categories.is_some() || req.star.is_some() {
        let add_tags = req.add_tags.clone();
        let star = req.star;
        let result = state
            .pipeline
            .submit_batch_metadata(ids, move |meta| {
                if let Some(tags) = &add_tags {
                    for t in tags {
                        if !meta.tags.contains(t) {
                            meta.tags.push(t.clone());
                        }
                    }
                }
                if let Some(categories) = &add_categories {
                    for c in categories {
                        if !meta.categories.contains(c) {
                            meta.categories.push(c.clone());
                        }
                    }
                }
                if let Some(star) = star {
                    meta.star = star;
                }
            })
            .await
            .map_err(ApiError::internal)?;
        updated = result.updated;
        missing.extend(result.missing_ids);
    }
    missing.dedup();

    Ok(Json(Envelope::ok(ItemBatchUpdateResponse {
        updated,
        missing_ids: missing,
    })))
}

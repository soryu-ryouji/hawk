//! item/upload：multipart 内容上传（web 端无文件路径可引用时的入库通道）。

use super::*;

// ---------- upload（web 端内容上传） ----------

/// multipart 表单描述（仅用于 OpenAPI；实现为流式 Multipart 提取）
#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
struct ItemUploadForm {
    /// 文件内容（必需；文件名只取末段防跨目录，扩展名决定类型）
    #[schema(value_type = String, format = Binary)]
    file: Vec<u8>,
    /// 目标文件夹（库内相对路径，缺省库根；不存在自动创建）
    folder_path: Option<String>,
    /// 文件名覆盖（不含扩展名；默认取 file 文件名）
    name: Option<String>,
    /// 内容已在库内（不含回收站）时跳过
    skip_existing: Option<bool>,
    /// 标签（可重复出现多次；缺省不设置）
    tags: Option<Vec<String>>,
    /// 分类（可重复出现多次；缺省不设置）
    categories: Option<Vec<String>>,
}

/// multipart/form-data 上传：浏览器/iOS 分享扩展无文件路径可引用（拿到的是内容），
/// 经本端点以内容入库。字段：file（二进制，必需）/ folder_path / name（可选，默认取 file 文件名）/
/// skip_existing（可选，内容已在库内时跳过）/ tags、categories（可选，可重复出现多次）。
/// 写权限：admin 恒可用；viewer 需 [web].writable（auth 中间件统一拦截）
#[utoipa::path(
    post,
    path = "/api/v1/item/upload",
    tags = ["item"],
    request_body(content = ItemUploadForm, content_type = "multipart/form-data"),
    responses((status = 200, description = "OK", body = Envelope<ItemAddResponse>))
)]
pub(crate) async fn item_upload(
    State(state): State<SharedState>,
    mut multipart: axum::extract::Multipart,
) -> Result<Json<Envelope<ItemAddResponse>>, ApiError> {
    let mut folder_rel = String::new();
    let mut name_override: Option<String> = None;
    let mut skip_existing = false;
    let mut file: Option<(String, Vec<u8>)> = None;
    let mut tags: Vec<String> = Vec::new();
    let mut categories: Vec<String> = Vec::new();
    let mut has_tags = false;
    let mut has_categories = false;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::invalid_param(format!("multipart 解析失败: {e}")))?
    {
        match field.name().unwrap_or_default() {
            "folder_path" => {
                folder_rel = field
                    .text()
                    .await
                    .map_err(|e| ApiError::invalid_param(format!("读取 folder_path 失败: {e}")))?;
            }
            "name" => {
                name_override = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| ApiError::invalid_param(format!("读取 name 失败: {e}")))?,
                );
            }
            "skip_existing" => {
                skip_existing = field.text().await.map_err(|e| {
                    ApiError::invalid_param(format!("读取 skip_existing 失败: {e}"))
                })? == "true";
            }
            // 分类/标签：同一字段名重复出现多次（multipart 无数组类型），空串忽略
            "tags" => {
                let value = field
                    .text()
                    .await
                    .map_err(|e| ApiError::invalid_param(format!("读取 tags 失败: {e}")))?;
                if !value.trim().is_empty() {
                    tags.push(value);
                }
                has_tags = true;
            }
            "categories" => {
                let value = field
                    .text()
                    .await
                    .map_err(|e| ApiError::invalid_param(format!("读取 categories 失败: {e}")))?;
                if !value.trim().is_empty() {
                    categories.push(value);
                }
                has_categories = true;
            }
            "file" => {
                // 文件名只取最后一段（防跨目录写入），扩展名决定入库类型（与 path 导入同语义：不眼内容）
                let raw = field.file_name().unwrap_or_default().to_string();
                let filename = std::path::Path::new(&raw)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::invalid_param(format!("读取文件内容失败: {e}")))?
                    .to_vec();
                file = Some((filename, bytes));
            }
            _ => {} // 未知字段忽略
        }
    }
    let Some((filename, bytes)) = file else {
        return Err(ApiError::invalid_param("缺少 file 字段"));
    };

    if !folder_rel.is_empty() && !LibraryPaths::is_valid_library_path(Some(&folder_rel)) {
        return Err(ApiError::invalid_param(format!(
            "非法文件夹路径: {folder_rel}"
        )));
    }
    let lib = state.require_library()?;
    let folder_abs = if folder_rel.is_empty() {
        lib.paths.root.clone()
    } else {
        lib.paths.to_absolute(&folder_rel).unwrap()
    };
    std::fs::create_dir_all(&folder_abs)
        .map_err(|e| ApiError::internal(format!("创建目标目录失败: {e}")))?;

    let stem = name_override.unwrap_or_else(|| LibraryPaths::name_of(&filename).to_string());
    if !fs_util::is_valid_name(Some(&stem)) {
        return Err(ApiError::invalid_param(format!("非法文件名: {filename}")));
    }
    let ext = LibraryPaths::ext_of(&filename);
    let file_name = if ext.is_empty() {
        stem.clone()
    } else {
        format!("{stem}.{ext}")
    };
    let target_rel = if folder_rel.is_empty() {
        file_name.clone()
    } else {
        format!("{folder_rel}/{file_name}")
    };
    // 入库策略前置校验（ignore 规则 / 扩展名白名单）：写盘前拒绝
    if lib.config.is_ignored(&target_rel) {
        return Err(ApiError::invalid_param(format!(
            "目标路径被 ignore 规则排除: {target_rel}"
        )));
    }
    if !lib.config.is_extension_included(&file_name) {
        return Err(ApiError::unsupported_format(format!(
            "扩展名不在素材库可见格式白名单内: {file_name}"
        )));
    }
    let target_abs = lib.paths.to_absolute(&target_rel).unwrap();
    if std::path::Path::new(&target_abs).exists() {
        return Err(ApiError::file_exists(&target_rel));
    }

    // 先算哈希判断内容是否已存在(already_existed 语义以写入前为准)
    let hash = content_hash::hash_bytes(&bytes);
    let existed_before_write = lib.index.contains(&hash);

    // skip_existing：内容已在库内则跳过（不写文件、不追加路径）；仅回收站存在时不跳过
    if skip_existing && lib.index.has_library_location(&hash) {
        let dto = lib
            .index
            .get_dto(&hash)
            .ok_or_else(|| ApiError::internal("索引失败"))?;
        return Ok(Json(Envelope::ok(ItemAddResponse {
            item: dto,
            already_existed: true,
            skipped: true,
        })));
    }

    // 文件落库（阻塞 IO 移出运行时线程）
    tokio::task::spawn_blocking({
        let target = target_abs.clone();
        move || std::fs::write(&target, &bytes).map_err(|e| format!("写入文件失败: {e}"))
    })
    .await
    .map_err(|e| ApiError::internal(format!("写入任务失败: {e}")))?
    .map_err(ApiError::internal)?;

    // 哈希已算好,流水线跳过重算,避免大文件导入时二次读盘
    lib.pipeline
        .submit_upsert(target_abs, Some(hash.clone()))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("索引失败"))?;

    // 附带的分类/标签写入元数据（与 item/add 同语义：分类名归一化去重，标签原样）
    if has_tags || has_categories {
        let normalized_categories = if has_categories {
            normalize_categories(Some(&categories))?
        } else {
            None
        };
        let tags = has_tags.then_some(tags);
        lib.pipeline
            .submit_metadata(hash.clone(), move |meta| {
                if let Some(tags) = tags {
                    meta.tags = tags;
                }
                if let Some(categories) = normalized_categories {
                    meta.categories = categories;
                }
            })
            .await
            .map_err(ApiError::internal)?;
    }

    // 元数据可能刚经 submit_metadata 更新，响应以最新投影为准
    let dto = lib
        .index
        .get_dto(&hash)
        .ok_or_else(|| ApiError::internal("索引失败"))?;
    Ok(Json(Envelope::ok(ItemAddResponse {
        item: dto,
        already_existed: existed_before_write,
        skipped: false,
    })))
}

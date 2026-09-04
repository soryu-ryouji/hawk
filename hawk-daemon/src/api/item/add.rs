//! item/add：路径导入与 URL 下载入库。

use super::*;

// ---------- add ----------

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemAddRequest {
    path: Option<String>,
    url: Option<String>,
    img_base64: Option<String>,
    name: Option<String>,
    folder_path: Option<String>,
    tags: Option<Vec<String>>,
    categories: Option<Vec<String>>,
    annotation: Option<String>,
    /// 来源网页(收集场景:图片所在的页面地址),记录为 Item.url;与下载用的 url 区分
    website: Option<String>,
    /// 内容已存在于库内（不含回收站）时跳过：不写文件、不追加路径，响应 skipped=true
    #[serde(default)]
    skip_existing: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemAddResponse {
    pub(crate) item: ItemDto,
    pub(crate) already_existed: bool,
    /// 内容已存在且按 skip_existing 跳过：未写入文件、未追加路径（already_existed 旧语义仍保留：
    /// 已写入并关联到既有条目）
    pub(crate) skipped: bool,
}

pub(crate) async fn item_add(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemAddRequest>,
) -> Result<Json<Envelope<ItemAddResponse>>, ApiError> {
    if req.path.is_none() && req.url.is_none() && req.img_base64.is_none() {
        return Err(ApiError::invalid_param("path、url、img_base64 必须提供其一"));
    }

    let folder_rel = req.folder_path.clone().unwrap_or_default();
    if !folder_rel.is_empty() && !LibraryPaths::is_valid_library_path(Some(&folder_rel)) {
        return Err(ApiError::invalid_param(format!("非法文件夹路径: {}", req.folder_path.unwrap_or_default())));
    }

    // 导入时目标目录不存在则自动创建
    let folder_abs = if folder_rel.is_empty() {
        state.paths.root.clone()
    } else {
        state.paths.to_absolute(&folder_rel).unwrap()
    };
    std::fs::create_dir_all(&folder_abs).map_err(|e| ApiError::internal(format!("创建目标目录失败: {e}")))?;

    // 获取内容来源:本地文件直接引用,url/base64 内容在内存中
    let (ext, default_name, bytes, source_abs): (String, String, Option<Vec<u8>>, Option<String>) =
        if let Some(path) = &req.path {
            let source = std::path::Path::new(path);
            if !source.is_file() {
                return Err(ApiError::invalid_param(format!("文件不存在: {path}")));
            }
            let file_name = source.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let ext = LibraryPaths::ext_of(&file_name);
            let stem = LibraryPaths::name_of(&file_name).to_string();
            (ext, stem, None, Some(crate::core::paths::full_path(path)))
        } else if let Some(url) = &req.url {
            let uri = url::Url::parse(url).map_err(|_| ApiError::invalid_param(format!("非法 URL: {url}")))?;
            let bytes = download(url).await?;
            let segment = uri
                .path_segments()
                .and_then(|s| s.last().map(str::to_string))
                .unwrap_or_default();
            let decoded = percent_decode(&segment);
            let ext = LibraryPaths::ext_of(&decoded);
            let ext = if ext.is_empty() {
                crate::core::thumbnail::ThumbnailService::detect_extension_bytes(&bytes)
                    .ok_or_else(|| ApiError::invalid_param("无法确定文件扩展名"))?
            } else {
                ext
            };
            let stem = LibraryPaths::name_of(&decoded).to_string();
            let default_name = if stem.is_empty() { "download".to_string() } else { stem };
            (ext, default_name, Some(bytes), None)
        } else {
            let bytes = decode_base64(req.img_base64.as_deref().unwrap_or_default())?;
            let ext = ThumbnailService::detect_extension_bytes(&bytes)
                .ok_or_else(|| ApiError::unsupported_format("无法识别的图像数据"))?;
            (ext, "image".to_string(), Some(bytes), None)
        };
    let name = req.name.clone().unwrap_or(default_name);
    if !fs_util::is_valid_name(Some(&name)) {
        return Err(ApiError::invalid_param(format!("非法文件名: {}", req.name.unwrap_or_default())));
    }

    let file_name = if ext.is_empty() { name.clone() } else { format!("{name}.{ext}") };
    let target_rel = if folder_rel.is_empty() {
        file_name.clone()
    } else {
        format!("{folder_rel}/{file_name}")
    };
    let target_abs = state.paths.to_absolute(&target_rel).unwrap();
    if std::path::Path::new(&target_abs).exists() {
        return Err(ApiError::file_exists(&target_rel));
    }

    // 先算哈希判断内容是否已存在(already_existed 语义以写入前为准)
    let hash = match (&source_abs, &bytes) {
        (Some(src), _) => content_hash::hash_file(src).map_err(|e| ApiError::internal(format!("计算哈希失败: {e}")))?,
        (None, Some(data)) => content_hash::hash_bytes(data),
        _ => unreachable!(),
    };
    let existed_before_write = state.index.contains(&hash);

    // skip_existing：内容已在库内（不含回收站——删掉的内容应可重新导入）则跳过，
    // 不写文件也不追加路径（多路径副本是重复导入的磁盘占用来源）
    if req.skip_existing && state.index.has_library_location(&hash) {
        let dto = state.index.get_dto(&hash).ok_or_else(|| ApiError::internal("索引失败"))?;
        return Ok(Json(Envelope::ok(ItemAddResponse {
            item: dto,
            already_existed: true,
            skipped: true,
        })));
    }

    // 文件落库（阻塞 IO 移出运行时线程）。path 导入保留原文件的创建时间与修改时间
    tokio::task::spawn_blocking({
        let src = source_abs.clone();
        let data = bytes.clone();
        let target = target_abs.clone();
        move || match (src.as_deref(), data.as_ref()) {
            (Some(src), _) => {
                std::fs::copy(src, &target).map_err(|e| format!("复制文件失败: {e}"))?;
                preserve_times(src, &target);
                Ok::<(), String>(())
            }
            (None, Some(data)) => std::fs::write(&target, data).map_err(|e| format!("写入文件失败: {e}")),
            _ => unreachable!(),
        }
    })
    .await
    .map_err(|e| ApiError::internal(format!("写入任务失败: {e}")))?
    .map_err(ApiError::internal)?;

    // 哈希已算好,流水线跳过重算,避免大文件导入时二次读盘
    let _result = state
        .pipeline
        .submit_upsert(target_abs.clone(), Some(hash.clone()))
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("索引失败"))?;

    // 附带的素材参数写入元数据;website(来源网页)记录为 Item.url,下载用的 url 不覆盖它
    if req.tags.is_some() || req.annotation.is_some() || req.website.is_some() || req.categories.is_some() {
        let categories = normalize_categories(req.categories.as_deref())?;
        let tags = req.tags.clone();
        let annotation = req.annotation.clone();
        let website = req.website.clone();
        state
            .pipeline
            .submit_metadata(hash.clone(), move |meta| {
                if let Some(tags) = tags {
                    meta.tags = tags;
                }
                if let Some(categories) = categories {
                    meta.categories = categories;
                }
                if let Some(annotation) = annotation {
                    meta.annotation = Some(annotation);
                }
                if let Some(website) = website {
                    meta.url = Some(website);
                }
            })
            .await
            .map_err(ApiError::internal)?;
    }

    // 元数据可能刚经 submit_metadata 更新,响应以最新投影为准
    let dto = state
        .index
        .get_dto(&hash)
        .ok_or_else(|| ApiError::internal("索引失败"))?;
    Ok(Json(Envelope::ok(ItemAddResponse {
        item: dto,
        already_existed: existed_before_write,
        skipped: false,
    })))
}

async fn download(url: &str) -> Result<Vec<u8>, ApiError> {
    let url = url.to_string();
    tokio::task::spawn_blocking(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .build()
            .into();
        let response = agent
            .get(&url)
            .call()
            .map_err(|e| ApiError::internal(format!("下载失败: {e}")))?;
        response
            .into_body()
            .read_to_vec()
            .map_err(|e| ApiError::internal(format!("读取下载内容失败: {e}")))
    })
    .await
    .map_err(|e| ApiError::internal(format!("下载任务失败: {e}")))?
}

fn percent_decode(input: &str) -> String {
    percent_encoding::percent_decode_str(input).decode_utf8_lossy().to_string()
}

/// path 导入保留原文件的创建时间与修改时间（File.Copy 默认会重置）
fn preserve_times(src: &str, dst: &str) {
    if let Ok(meta) = std::fs::metadata(src) {
        if let Ok(mtime) = meta.modified() {
            let _ = filetime::set_file_mtime(dst, filetime::FileTime::from_system_time(mtime));
        }
        if let Ok(atime) = meta.accessed() {
            let _ = filetime::set_file_atime(dst, filetime::FileTime::from_system_time(atime));
        }
    }
}

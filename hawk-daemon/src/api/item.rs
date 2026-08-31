//! item 端点（12 个）：list/skeleton/detail/count/add/update/batch_update/delete/restore/
//! thumbnail/file/refresh_thumbnail/replace。
//! 写路径的真实文件操作在本层完成，随后提交索引流水线并等待完成；读取一律走索引锁内投影。

use crate::api::envelope::{success, ApiError, Envelope, JsonBody};
use crate::api::SharedState;
use crate::core::color_math;
use crate::core::content_hash;
use crate::core::fs_util;
use crate::core::index::LocationSnapshot;
use crate::core::item::{ItemDto, ItemQuery, ItemSkeletonDto};
use crate::core::paths::LibraryPaths;
use crate::core::thumbnail::ThumbnailService;
use axum::extract::{Query, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde::Serialize;

pub fn routes() -> Router<SharedState> {
    Router::new()
        .route("/api/v1/item/list", post(item_list))
        .route("/api/v1/item/skeleton", post(item_skeleton))
        .route("/api/v1/item/detail", get(item_detail))
        .route("/api/v1/item/count", get(item_count))
        .route("/api/v1/item/add", post(item_add))
        .route("/api/v1/item/update", post(item_update))
        .route("/api/v1/item/batch_update", post(item_batch_update))
        .route("/api/v1/item/delete", post(item_delete))
        .route("/api/v1/item/restore", post(item_restore))
        .route("/api/v1/item/thumbnail", get(item_thumbnail))
        .route("/api/v1/item/file", get(item_file))
        .route("/api/v1/item/refresh_thumbnail", post(item_refresh_thumbnail))
        .route("/api/v1/item/replace", post(item_replace))
}

// ---------- list / skeleton / detail / count ----------

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", default)]
struct ItemListRequest {
    ids: Option<Vec<String>>,
    keywords: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    star: Option<i32>,
    folders: Option<Vec<String>>,
    folders_exact: bool,
    categories: Option<Vec<String>>,
    categories_match: Option<String>,
    exclude_categories: Option<Vec<String>>,
    exclude_tags: Option<Vec<String>>,
    without_categories: bool,
    without_tags: bool,
    ext: Option<String>,
    annotation: Option<String>,
    url: Option<String>,
    color: Option<String>,
    in_trash: bool,
    order_by: Option<String>,
    order: Option<String>,
    offset: i32,
    limit: i32,
}

impl Default for ItemListRequest {
    fn default() -> Self {
        ItemListRequest {
            ids: None,
            keywords: None,
            tags: None,
            star: None,
            folders: None,
            folders_exact: false,
            categories: None,
            categories_match: None,
            exclude_categories: None,
            exclude_tags: None,
            without_categories: false,
            without_tags: false,
            ext: None,
            annotation: None,
            url: None,
            color: None,
            in_trash: false,
            order_by: None,
            order: None,
            offset: 0,
            limit: 50,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct ItemListResponse {
    items: Vec<ItemDto>,
    total: usize,
    total_size: i64,
    offset: i32,
    limit: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct ItemSkeletonResponse {
    items: Vec<ItemSkeletonDto>,
    total_size: i64,
}

/// ItemListRequest → ItemQuery：/list 与 /skeleton 必须走同一条路径,保证两次查询次序逐位一致
fn build_query(req: ItemListRequest) -> Result<ItemQuery, ApiError> {
    let color = match &req.color {
        Some(c) => {
            let (r, g, b) = color_math::parse_hex(Some(c))
                .ok_or_else(|| ApiError::invalid_param(format!("非法颜色值: {c}")))?;
            Some(color_math::rgb_to_lab(r, g, b))
        }
        None => None,
    };
    Ok(ItemQuery {
        ids: req.ids,
        keywords: req.keywords,
        tags: req.tags,
        star: req.star,
        folders: req.folders,
        folders_exact: req.folders_exact,
        categories: req.categories,
        categories_match: req.categories_match,
        exclude_categories: req.exclude_categories,
        exclude_tags: req.exclude_tags,
        without_categories: req.without_categories,
        without_tags: req.without_tags,
        ext: req.ext,
        annotation: req.annotation,
        url: req.url,
        color,
        in_trash: req.in_trash,
        order_by: req.order_by,
        order: req.order,
        offset: req.offset,
        limit: req.limit.max(1),
    })
}

async fn item_list(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemListRequest>,
) -> Result<Json<Envelope<ItemListResponse>>, ApiError> {
    let query = build_query(req)?;
    let (items, total, total_size) = state.index.query(&query);
    dispatch_dim_heal(&state, items.iter().map(|i| (i.id.as_str(), i.width)));
    Ok(Json(Envelope::ok(ItemListResponse {
        items,
        total,
        total_size,
        offset: query.offset,
        limit: query.limit,
    })))
}

async fn item_skeleton(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemListRequest>,
) -> Result<Json<Envelope<ItemSkeletonResponse>>, ApiError> {
    let query = build_query(req)?;
    let (items, total_size) = state.index.query_skeleton(&query);
    dispatch_dim_heal(&state, items.iter().map(|i| (i.id.as_str(), i.width)));
    Ok(Json(Envelope::ok(ItemSkeletonResponse { items, total_size })))
}

/// 读取端宽高自愈：响应中发现 0 × 0 的 item → 派发后台补全任务（identify 补宽高 + 按需调色板）。
/// 入库时解码暂时失败会把 width=0 落库且无事件再触及，用户拉列表即触发重试，
/// 修复后经 item.updated 事件自动刷新骨架/卡片。in-flight 去重，幂等，高频调用零负担
fn dispatch_dim_heal<'a>(state: &SharedState, items: impl Iterator<Item = (&'a str, i32)>) {
    for (hash, width) in items {
        if width == 0 {
            if let Some(abs) = state.index.main_source_abs(hash, &state.paths) {
                state.worker.enqueue_palette(hash, &abs);
            }
        }
    }
}

#[derive(Deserialize)]
struct IdQuery {
    id: String,
}

async fn item_detail(
    State(state): State<SharedState>,
    Query(q): Query<IdQuery>,
) -> Result<Json<Envelope<ItemDto>>, ApiError> {
    let dto = state
        .index
        .get_dto(&q.id)
        .ok_or_else(|| ApiError::item_not_found(&q.id))?;
    Ok(Json(Envelope::ok(dto)))
}

async fn item_count(State(state): State<SharedState>) -> Json<Envelope<usize>> {
    Json(Envelope::ok(state.index.count()))
}

// ---------- add ----------

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct ItemAddRequest {
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
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct ItemAddResponse {
    item: ItemDto,
    already_existed: bool,
}

async fn item_add(
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

fn decode_base64(input: &str) -> Result<Vec<u8>, ApiError> {
    use base64::Engine;
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|_| ApiError::invalid_param("img_base64 不是合法的 Base64 数据"))
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

fn normalize_categories(raw: Option<&[String]>) -> Result<Option<Vec<String>>, ApiError> {
    let Some(raw) = raw else { return Ok(None) };
    let mut out = Vec::new();
    for name in raw {
        let normalized = crate::core::taxonomy::normalize_category_name(Some(name))
            .ok_or_else(|| ApiError::invalid_param("包含非法分类名称"))?;
        if !out.contains(&normalized) {
            out.push(normalized);
        }
    }
    Ok(Some(out))
}

// ---------- update ----------

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct ItemUpdateRequest {
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

async fn item_update(
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

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct ItemBatchUpdateRequest {
    ids: Vec<String>,
    add_tags: Option<Vec<String>>,
    add_categories: Option<Vec<String>>,
    star: Option<i32>,
    folder_path: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct ItemBatchUpdateResponse {
    updated: usize,
    missing_ids: Vec<String>,
}

async fn item_batch_update(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemBatchUpdateRequest>,
) -> Result<Json<Envelope<ItemBatchUpdateResponse>>, ApiError> {
    if req.ids.is_empty() {
        return Err(ApiError::invalid_param("ids 不能为空"));
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

    // folder_path:逐个移动主位置(库内);已在目标处的跳过;无库内位置(全在回收站)的移动不适用,跳过
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
        for id in &ids {
            let Some(loc) = state.index.find_location(id, None, Some(false)) else {
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

// ---------- delete / restore ----------

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct ItemIdRequest {
    id: String,
    path: Option<String>,
}

async fn item_delete(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemIdRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let loc = state
        .index
        .find_location(&req.id, req.path.as_deref(), Some(false))
        .ok_or_else(|| {
            ApiError::invalid_param(if req.path.is_none() {
                "item 不在库内".to_string()
            } else {
                format!("库内不存在该文件位置: {}", req.path.unwrap_or_default())
            })
        })?;

    let source_abs = state.paths.to_absolute(&loc.path).unwrap();
    let trash_abs = fs_util::find_free_trash_path(&state.paths, &loc.path, false);
    fs_util::ensure_parent_dir(&trash_abs);
    std::fs::rename(&source_abs, &trash_abs).map_err(|e| ApiError::internal(format!("移入回收站失败: {e}")))?;
    state.pipeline.submit_move(source_abs, trash_abs).await.map_err(ApiError::internal)?;
    Ok(Json(success()))
}

async fn item_restore(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemIdRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let loc = state
        .index
        .find_location(&req.id, req.path.as_deref(), Some(true))
        .ok_or_else(|| {
            ApiError::invalid_param(if req.path.is_none() {
                "item 不在回收站".to_string()
            } else {
                format!("回收站中不存在该文件位置: {}", req.path.unwrap_or_default())
            })
        })?;

    // 按原路径放回(回收站中的实际名称去掉 .hawk/trash/ 前缀)
    let target_abs = state.paths.to_absolute(&loc.library_path).unwrap();
    if std::path::Path::new(&target_abs).exists() {
        return Err(ApiError::file_exists(&loc.library_path));
    }
    let source_abs = state.paths.to_absolute(&loc.path).unwrap();
    fs_util::ensure_parent_dir(&target_abs);
    std::fs::rename(&source_abs, &target_abs).map_err(|e| ApiError::internal(format!("恢复失败: {e}")))?;
    state.pipeline.submit_move(source_abs, target_abs).await.map_err(ApiError::internal)?;
    Ok(Json(success()))
}

// ---------- thumbnail / file / refresh_thumbnail ----------

#[derive(Deserialize)]
struct ThumbnailQuery {
    id: String,
    size: Option<i32>,
}

async fn item_thumbnail(
    State(state): State<SharedState>,
    Query(q): Query<ThumbnailQuery>,
) -> Result<Response, ApiError> {
    let actual_size = q.size.unwrap_or(256);
    if !state.config.current().thumbnail_sizes.contains(&actual_size) {
        return Err(ApiError::invalid_param(format!("不支持的缩略图尺寸: {actual_size}")));
    }
    if !state.index.contains(&q.id) {
        return Err(ApiError::item_not_found(&q.id));
    }
    let file = state.thumbs.get_path(&q.id, actual_size);
    if std::path::Path::new(&file).is_file() {
        return serve_file(file, "image/webp".to_string(), true).await;
    }

    // 未命中（缩略图为惰性缓存，入库/对账不生成）：
    // - 浏览器可渲染且可解码 → 直接回源原图（首次查看零等待），同时后台入队生成缓存
    // - 不可渲染格式（tiff 等）→ 后台生成后以 404 占位，经 item.updated 重建（现有闭环）
    // 入队以 identify（只解码头）为闸，避免不可解码内容被反复入队空转
    let source = state
        .index
        .main_source_abs(&q.id, &state.paths)
        .filter(|p| std::path::Path::new(p).is_file())
        .ok_or_else(|| ApiError::item_not_found(format!("thumbnail {} ({})", q.id, actual_size)))?;
    let decodable = ThumbnailService::identify(&source).is_some();
    if ThumbnailService::is_browser_renderable(&source) && decodable {
        state.worker.enqueue_thumbs(&q.id, &source);
        let content_type = mime_guess::from_path(&source).first_or_octet_stream().to_string();
        return serve_file(source, content_type, true).await;
    }
    if decodable {
        state.worker.enqueue_thumbs(&q.id, &source);
    }
    Err(ApiError::item_not_found(format!("thumbnail {} ({})", q.id, actual_size)))
}

async fn item_file(State(state): State<SharedState>, Query(q): Query<IdQuery>) -> Result<Response, ApiError> {
    let file = state
        .index
        .main_source_abs(&q.id, &state.paths)
        .ok_or_else(|| ApiError::item_not_found(&q.id))?;
    if !std::path::Path::new(&file).is_file() {
        return Err(ApiError::item_not_found(format!("file {}", q.id)));
    }
    let content_type = mime_guess::from_path(&file).first_or_octet_stream().to_string();
    serve_file(file, content_type, true).await
}

async fn serve_file(file: String, content_type: String, immutable: bool) -> Result<Response, ApiError> {
    use tokio::io::AsyncReadExt;
    let f = tokio::fs::File::open(&file)
        .await
        .map_err(|e| ApiError::internal(format!("读取文件失败: {e}")))?;
    // 流式返回（128KB 块）：大文件不整读进内存，也不阻塞运行时线程；
    // 读取中途出错以流错误终止响应（客户端感知截断），与整读失败的可见性一致
    let stream = async_stream::stream! {
        let mut reader = f;
        let mut buf = vec![0u8; 128 * 1024];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => yield Ok::<axum::body::Bytes, std::io::Error>(axum::body::Bytes::copy_from_slice(&buf[..n])),
                Err(e) => {
                    yield Err(e);
                    break;
                }
            }
        }
    };
    let mut builder = Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", content_type);
    if immutable {
        // item id 是内容哈希，内容永不变，客户端可永久缓存
        builder = builder.header("cache-control", "public, max-age=31536000, immutable");
    }
    Ok(builder.body(axum::body::Body::from_stream(stream)).unwrap())
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct ItemRefreshThumbnailRequest {
    id: String,
}

async fn item_refresh_thumbnail(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemRefreshThumbnailRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let source = state
        .index
        .main_source_abs(&req.id, &state.paths)
        .ok_or_else(|| ApiError::item_not_found(&req.id))?;
    // 手动强制重建：走 worker 任务（强制重建全部尺寸 + 补宽高/调色板），
    // 完成后经 item.updated 通知前端重建 <img>；直接调 generate 不回写宽高也不发事件
    state.worker.enqueue_force_rebuild(&req.id, &source);
    Ok(Json(success()))
}

// ---------- replace ----------

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct ItemReplaceRequest {
    id: String,
    path: Option<String>,
    img_base64: String,
}

/// 内容替换(item/replace):客户端编辑(旋转/裁切等)后的新内容提交存储层。
/// 哈希变化 → id 漂移,元数据继承迁移/事件/缩略图重建由索引流水线闭环。
/// 写回时保留原文件的修改时间（修正性编辑不改变素材的时序位置）
async fn item_replace(
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

// ---------- 公共辅助 ----------

fn find_location(
    state: &SharedState,
    id: &str,
    path: Option<&str>,
    want_trash: Option<bool>,
) -> Result<LocationSnapshot, ApiError> {
    state
        .index
        .find_location(id, path, want_trash)
        .ok_or_else(|| ApiError::item_not_found(path.unwrap_or(id)))
}

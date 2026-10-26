//! 内容服务与缩略图：thumbnail/file/refresh_thumbnail。
//! 锁强制：两个内容端点按 item 级公式判定（位置 OR + 分类/标签 AND），未解锁 403 LOCKED——
//! 这是防止「拿到 hash 直接拼 URL 绕过列表」的关键闸门

use super::*;
use crate::core::locks::LockGuard;
use axum::extract::Extension;

// ---------- thumbnail / file / refresh_thumbnail ----------

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ThumbnailQuery {
    /// item id（内容 BLAKE3 哈希 hex）
    id: String,
    /// 缩略图边长：512（网格）或 1024（检查器/预览），缺省 1024
    size: Option<u32>,
}

/// 锁闸门：item 级可见性判定（位置 OR + 分类/标签 AND）。不可见 → 403 LOCKED。
/// 缩略图与原图共用；守卫空（无锁/全解锁）时零成本直通
fn check_item_lock(
    lib: &crate::api::LibraryState,
    guard: &LockGuard,
    id: &str,
) -> Result<(), ApiError> {
    if guard.is_empty() {
        return Ok(());
    }
    if let Some((paths, categories, tags)) = lib.index.lock_projection(id) {
        if !guard.item_visible(paths.into_iter(), &categories, &tags) {
            return Err(ApiError::locked("内容已锁定，需要解锁后访问"));
        }
    }
    Ok(())
}

/// 缩略图：固定 512/1024 两档 WebP 缓存，Cache-Control immutable。
/// 未命中时后台入队生成并返回 404，完成经 item.updated 通知客户端重试；
/// 不回源原图，避免低带宽下一张卡片下载整份原图，也避免 immutable 缓存钉住回退响应。
#[utoipa::path(
    get,
    path = "/api/v1/item/thumbnail",
    tags = ["item"],
    params(ThumbnailQuery),
    responses(
        (status = 200, description = "指定尺寸的 WebP 缩略图", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 400, description = "size 不是受支持的尺寸"),
        (status = 403, description = "内容位于未解锁的锁覆盖内（LOCKED）"),
        (status = 404, description = "缩略图生成中或源图不可解码")
    )
)]
pub(crate) async fn item_thumbnail(
    State(state): State<SharedState>,
    Extension(guard): Extension<LockGuard>,
    Query(q): Query<ThumbnailQuery>,
) -> Result<Response, ApiError> {
    let lib = state.require_library()?;
    if !lib.index.contains(&q.id) {
        return Err(ApiError::item_not_found(&q.id));
    }
    check_item_lock(lib, &guard, &q.id)?;
    let size = q
        .size
        .unwrap_or(crate::core::thumbnail::DEFAULT_THUMBNAIL_SIZE);
    if !crate::core::thumbnail::is_supported_size(size) {
        return Err(ApiError::invalid_param(
            "size 仅支持 512 或 1024".to_string(),
        ));
    }
    let file = lib.thumbs.get_path_with_size(&q.id, size);
    if std::path::Path::new(&file).is_file() {
        return serve_file(file, "image/webp".to_string()).await;
    }

    // 未命中（增量入库的缩略图为惰性缓存）：入队生成全部缺失档，前端经 item.updated 重试。
    // 入队以 identify（只解码头）为闸，避免不可解码内容被反复入队空转
    let source = lib
        .index
        .main_source_abs(&q.id, &lib.paths)
        .filter(|p| std::path::Path::new(p).is_file())
        .ok_or_else(|| ApiError::item_not_found(format!("thumbnail {}", q.id)))?;
    if ThumbnailService::identify(&source).is_some() {
        lib.worker.enqueue_thumbs(&q.id, &source);
    }
    Err(ApiError::item_not_found(format!("thumbnail {}", q.id)))
}

/// 主位置（优先库内）原图二进制：流式返回，Content-Type 按扩展名 mime_guess，Cache-Control immutable
#[utoipa::path(
    get,
    path = "/api/v1/item/file",
    tags = ["item"],
    params(IdQuery),
    responses(
        (status = 200, description = "原图二进制", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 403, description = "内容位于未解锁的锁覆盖内（LOCKED）")
    )
)]
pub(crate) async fn item_file(
    State(state): State<SharedState>,
    Extension(guard): Extension<LockGuard>,
    Query(q): Query<IdQuery>,
) -> Result<Response, ApiError> {
    let lib = state.require_library()?;
    check_item_lock(lib, &guard, &q.id)?;
    let file = lib
        .index
        .main_source_abs(&q.id, &lib.paths)
        .ok_or_else(|| ApiError::item_not_found(&q.id))?;
    if !std::path::Path::new(&file).is_file() {
        return Err(ApiError::item_not_found(format!("file {}", q.id)));
    }
    let content_type = mime_guess::from_path(&file)
        .first_or_octet_stream()
        .to_string();
    serve_file(file, content_type).await
}

async fn serve_file(file: String, content_type: String) -> Result<Response, ApiError> {
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
    // item id 是内容哈希，且缩略图命中后表示固定尺寸，响应均可永久缓存
    Ok(Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", content_type)
        .header("cache-control", "public, max-age=31536000, immutable")
        .body(axum::body::Body::from_stream(stream))
        .unwrap())
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemRefreshThumbnailRequest {
    id: String,
}

/// 强制重建缩略图（取可读主位置；完成后经 item.updated 通知前端重建 <img>）
#[utoipa::path(
    post,
    path = "/api/v1/item/refresh_thumbnail",
    tags = ["item"],
    request_body = ItemRefreshThumbnailRequest,
    responses((status = 200, description = "OK", body = SuccessOnly))
)]
pub(crate) async fn item_refresh_thumbnail(
    State(state): State<SharedState>,
    JsonBody(req): JsonBody<ItemRefreshThumbnailRequest>,
) -> Result<Json<SuccessOnly>, ApiError> {
    let lib = state.require_library()?;
    let source = lib
        .index
        .main_source_abs(&req.id, &lib.paths)
        .ok_or_else(|| ApiError::item_not_found(&req.id))?;
    // 手动强制重建：走 worker 任务（强制重建全部尺寸 + 补宽高/调色板），
    // 完成后经 item.updated 通知前端重建 <img>；直接调 generate 不回写宽高也不发事件
    lib.worker.enqueue_force_rebuild(&req.id, &source);
    Ok(success())
}

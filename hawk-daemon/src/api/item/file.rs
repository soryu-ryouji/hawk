//! 内容服务与缩略图：thumbnail/file/refresh_thumbnail。

use super::*;

// ---------- thumbnail / file / refresh_thumbnail ----------

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct ThumbnailQuery {
    /// item id（内容 BLAKE3 哈希 hex）
    id: String,
}

/// 缩略图：单一尺寸 1024 的 webp 缓存，Cache-Control immutable。
/// 未命中且浏览器可渲染（jpg/png/gif/webp/bmp）→ 直接回源原图（200，后台入队生成缓存）；
/// 不可渲染格式（tiff 等）生成中 404（经 item.updated 重建后可用）
#[utoipa::path(
    get,
    path = "/api/v1/item/thumbnail",
    tags = ["item"],
    params(ThumbnailQuery),
    responses(
        (status = 200, description = "缩略图（webp）或回源原图（Content-Type 按源格式）", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 404, description = "不可渲染格式，生成中")
    )
)]
pub(crate) async fn item_thumbnail(
    State(state): State<SharedState>,
    Query(q): Query<ThumbnailQuery>,
) -> Result<Response, ApiError> {
    if !state.index.contains(&q.id) {
        return Err(ApiError::item_not_found(&q.id));
    }
    let file = state.thumbs.get_path(&q.id);
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
        .ok_or_else(|| ApiError::item_not_found(format!("thumbnail {}", q.id)))?;
    let decodable = ThumbnailService::identify(&source).is_some();
    if ThumbnailService::is_browser_renderable(&source) && decodable {
        state.worker.enqueue_thumbs(&q.id, &source);
        let content_type = mime_guess::from_path(&source).first_or_octet_stream().to_string();
        return serve_file(source, content_type, true).await;
    }
    if decodable {
        state.worker.enqueue_thumbs(&q.id, &source);
    }
    Err(ApiError::item_not_found(format!("thumbnail {}", q.id)))
}

/// 主位置（优先库内）原图二进制：流式返回，Content-Type 按扩展名 mime_guess，Cache-Control immutable
#[utoipa::path(
    get,
    path = "/api/v1/item/file",
    tags = ["item"],
    params(IdQuery),
    responses((status = 200, description = "原图二进制", content_type = "application/octet-stream", body = Vec<u8>))
)]
pub(crate) async fn item_file(State(state): State<SharedState>, Query(q): Query<IdQuery>) -> Result<Response, ApiError> {
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
    let source = state
        .index
        .main_source_abs(&req.id, &state.paths)
        .ok_or_else(|| ApiError::item_not_found(&req.id))?;
    // 手动强制重建：走 worker 任务（强制重建全部尺寸 + 补宽高/调色板），
    // 完成后经 item.updated 通知前端重建 <img>；直接调 generate 不回写宽高也不发事件
    state.worker.enqueue_force_rebuild(&req.id, &source);
    Ok(success())
}

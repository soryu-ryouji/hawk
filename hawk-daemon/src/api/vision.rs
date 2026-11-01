//! 可选视觉扩展管理与本机构图检索。

use crate::api::envelope::{ApiError, Envelope};
use crate::api::{require_manage, AccessContext, SharedState};
use crate::core::item::ItemDto;
use crate::core::locks::LockGuard;
use crate::core::vision::VisionStatus;
use axum::extract::{Extension, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(install))
        .routes(routes!(enable))
        .routes(routes!(remove))
        .routes(routes!(search))
}

#[utoipa::path(
    get,
    path = "/api/v1/vision/status",
    tags = ["vision"],
    responses((status = 200, description = "视觉扩展状态", body = Envelope<VisionStatus>))
)]
async fn status(
    State(state): State<SharedState>,
) -> Result<Json<Envelope<VisionStatus>>, ApiError> {
    let lib = state.require_library()?;
    Ok(Json(Envelope::ok(lib.vision.status())))
}

#[utoipa::path(
    post,
    path = "/api/v1/vision/install",
    tags = ["vision"],
    responses((status = 200, description = "下载并启用视觉扩展", body = Envelope<VisionStatus>))
)]
async fn install(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
) -> Result<Json<Envelope<VisionStatus>>, ApiError> {
    require_manage(&access)?;
    let lib = state.require_library()?.clone();
    let vision = lib.vision.clone();
    // 下载与索引构建都是长任务：立即返回当前状态，客户端经 status 轮询进度
    tokio::spawn(async move {
        if let Err(e) = vision.install().await {
            tracing::error!("视觉扩展安装失败: {e}");
        }
    });
    Ok(Json(Envelope::ok(lib.vision.status())))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct VisionEnableRequest {
    enabled: bool,
}

#[utoipa::path(
    post,
    path = "/api/v1/vision/enable",
    tags = ["vision"],
    request_body = VisionEnableRequest,
    responses((status = 200, description = "启用或停用视觉扩展", body = Envelope<VisionStatus>))
)]
async fn enable(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
    Json(req): Json<VisionEnableRequest>,
) -> Result<Json<Envelope<VisionStatus>>, ApiError> {
    require_manage(&access)?;
    let lib = state.require_library()?;
    lib.vision
        .set_enabled(req.enabled)
        .map_err(ApiError::invalid_param)?;
    Ok(Json(Envelope::ok(lib.vision.status())))
}

#[utoipa::path(
    post,
    path = "/api/v1/vision/remove",
    tags = ["vision"],
    responses((status = 200, description = "删除视觉模型", body = Envelope<VisionStatus>))
)]
async fn remove(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
) -> Result<Json<Envelope<VisionStatus>>, ApiError> {
    require_manage(&access)?;
    let lib = state.require_library()?;
    lib.vision.remove().map_err(ApiError::invalid_param)?;
    Ok(Json(Envelope::ok(lib.vision.status())))
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", default)]
struct VisionSearchRequest {
    id: String,
    /// 最低余弦相似度（-1–1）；常用正值越高越严格
    min_similarity: f32,
    offset: i32,
    limit: i32,
}

impl Default for VisionSearchRequest {
    fn default() -> Self {
        Self {
            id: String::new(),
            min_similarity: 0.7,
            offset: 0,
            limit: 60,
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
struct VisionSearchEntryDto {
    item: ItemDto,
    similarity_score: f32,
}

#[derive(Serialize, utoipa::ToSchema)]
struct VisionSearchResponse {
    items: Vec<VisionSearchEntryDto>,
    total: usize,
    offset: i32,
    limit: i32,
}

#[utoipa::path(
    post,
    path = "/api/v1/item/vision-search",
    tags = ["item", "vision"],
    request_body = VisionSearchRequest,
    responses(
        (status = 200, description = "相似构图结果", body = Envelope<VisionSearchResponse>),
        (status = 400, description = "扩展未启用、源图未分析或参数非法")
    )
)]
async fn search(
    State(state): State<SharedState>,
    Extension(guard): Extension<LockGuard>,
    Json(req): Json<VisionSearchRequest>,
) -> Result<Json<Envelope<VisionSearchResponse>>, ApiError> {
    if req.id.is_empty()
        || !req.min_similarity.is_finite()
        || !(-1.0..=1.0).contains(&req.min_similarity)
    {
        return Err(ApiError::invalid_param(
            "id 不能为空，min_similarity 必须在 -1–1 之间",
        ));
    }
    let lib = state.require_library()?;
    if !lib.index.contains(&req.id) {
        return Err(ApiError::item_not_found(&req.id));
    }
    if let Some((paths, categories, tags)) = lib.index.lock_projection(&req.id) {
        if !guard.item_visible(paths.into_iter(), &categories, &tags) {
            return Err(ApiError::locked("内容已锁定，需要解锁后访问"));
        }
    }
    let matches = lib
        .vision
        .search(&req.id, req.min_similarity)
        .map_err(|message| {
            ApiError::new(
                crate::api::envelope::codes::NOT_READY,
                axum::http::StatusCode::CONFLICT,
                message,
            )
        })?;
    let mut entries = Vec::new();
    for (id, score) in matches {
        let visible = lib
            .index
            .lock_projection(&id)
            .is_none_or(|(paths, categories, tags)| {
                guard.item_visible(paths.into_iter(), &categories, &tags)
            });
        if !visible {
            continue;
        }
        let Some(item) = lib.index.get_dto(&id) else {
            continue;
        };
        entries.push(VisionSearchEntryDto {
            item,
            similarity_score: score,
        });
    }
    let total = entries.len();
    let offset = req.offset.max(0) as usize;
    let limit = req.limit.clamp(1, 100);
    let items = entries
        .into_iter()
        .skip(offset)
        .take(limit as usize)
        .collect();
    Ok(Json(Envelope::ok(VisionSearchResponse {
        items,
        total,
        offset: req.offset.max(0),
        limit,
    })))
}

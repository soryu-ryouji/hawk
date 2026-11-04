//! 重复图片扫描：按内容 hash 分组同一 item 的多个库内位置，admin 管理入口。
//! 只返回分组和位置信息；删除仍走 item/delete?id+path，避免整 item 误删。

use super::*;
use crate::api::{require_manage, AccessContext};
use crate::core::duplicates::{exact_groups, DuplicateGroup as CoreGroup};
use crate::core::locks::LockGuard;
use axum::extract::Extension;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct DuplicateScanRequest {
    offset: i32,
    limit: i32,
}

impl Default for DuplicateScanRequest {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 50,
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct DuplicateEntryDto {
    item: ItemDto,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct DuplicateGroupDto {
    /// 稳定组标识：内容 hash（item id）
    id: String,
    entries: Vec<DuplicateEntryDto>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct DuplicateScanResponse {
    groups: Vec<DuplicateGroupDto>,
    /// 过滤后的总组数（分页前）
    total: usize,
    offset: i32,
    limit: i32,
}

/// 重复图片分组扫描：同一内容 hash 的多个库内位置，分页按组切片
#[utoipa::path(
    post,
    path = "/api/v1/item/duplicates",
    tags = ["item"],
    request_body = DuplicateScanRequest,
    responses(
        (status = 200, description = "OK", body = Envelope<DuplicateScanResponse>),
        (status = 403, description = "仅 admin 可执行重复扫描")
    )
)]
pub(crate) async fn item_duplicates(
    State(state): State<SharedState>,
    Extension(access): Extension<AccessContext>,
    Extension(guard): Extension<LockGuard>,
    JsonBody(req): JsonBody<DuplicateScanRequest>,
) -> Result<Json<Envelope<DuplicateScanResponse>>, ApiError> {
    require_manage(&access)?;
    let lib = state.require_library()?.clone();
    let response = tokio::task::spawn_blocking(move || scan(lib, guard, req))
        .await
        .map_err(|e| ApiError::internal(format!("重复扫描任务失败: {e}")))?;
    Ok(Json(Envelope::ok(response)))
}

fn scan(
    lib: std::sync::Arc<crate::api::LibraryState>,
    guard: LockGuard,
    req: DuplicateScanRequest,
) -> DuplicateScanResponse {
    let items = lib.index.duplicate_items(&guard);
    let groups = exact_groups(&items);
    let total = groups.len();
    let offset = req.offset.max(0) as usize;
    let limit = req.limit.clamp(1, 100) as usize;
    let groups = groups
        .into_iter()
        .skip(offset)
        .take(limit)
        .filter_map(|group| project_group(&lib, group))
        .collect();
    DuplicateScanResponse {
        groups,
        total,
        offset: req.offset.max(0),
        limit: req.limit.clamp(1, 100),
    }
}

fn project_group(lib: &crate::api::LibraryState, group: CoreGroup) -> Option<DuplicateGroupDto> {
    let entries: Vec<DuplicateEntryDto> = group
        .members
        .into_iter()
        .filter_map(|member| {
            let item = lib.index.get_dto_at(&member.id, Some(&member.path))?;
            Some(DuplicateEntryDto { item })
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    Some(DuplicateGroupDto {
        id: group.id,
        entries,
    })
}

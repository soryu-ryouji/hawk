//! 重复图片扫描：相同内容多位置与 pHash 相似分组，admin 管理入口。
//! 只返回分组和位置信息；删除仍走 item/delete?id+path，避免整 item 误删。

use super::*;
use crate::api::{require_manage, AccessContext};
use crate::core::duplicates::{exact_groups, similar_groups, DuplicateGroup as CoreGroup};
use crate::core::locks::LockGuard;
use axum::extract::Extension;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct DuplicateScanRequest {
    /// similar（pHash 近重复）/ exact（同内容多位置）
    kind: String,
    /// 仅 similar：最大汉明距离（0–64，缺省 5）
    max_distance: Option<u32>,
    offset: i32,
    limit: i32,
}

impl Default for DuplicateScanRequest {
    fn default() -> Self {
        Self {
            kind: "similar".to_string(),
            max_distance: None,
            offset: 0,
            limit: 50,
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct DuplicateEntryDto {
    item: ItemDto,
    /// 与组内代表图的 pHash 汉明距离；exact 组不返回
    #[serde(skip_serializing_if = "Option::is_none")]
    similarity_distance: Option<u32>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct DuplicateGroupDto {
    /// 稳定组标识：exact 为 item id，similar 为组内代表 item id
    id: String,
    kind: String,
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

/// 重复图片分组扫描：分页按组切片，相同内容与相似图片共用响应结构
#[utoipa::path(
    post,
    path = "/api/v1/item/duplicates",
    tags = ["item"],
    request_body = DuplicateScanRequest,
    responses(
        (status = 200, description = "OK", body = Envelope<DuplicateScanResponse>),
        (status = 400, description = "kind 或 max_distance 非法"),
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
    if req.kind != "similar" && req.kind != "exact" {
        return Err(ApiError::invalid_param("kind 仅支持 similar 或 exact"));
    }
    if req.max_distance.unwrap_or(5) > 64 {
        return Err(ApiError::invalid_param("max_distance 必须在 0–64 之间"));
    }
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
    let groups = if req.kind == "exact" {
        exact_groups(&items)
    } else {
        similar_groups(&items, req.max_distance.unwrap_or(5))
    };
    let total = groups.len();
    let offset = req.offset.max(0) as usize;
    let limit = req.limit.clamp(1, 100) as usize;
    let kind = req.kind.as_str();
    let groups = groups
        .into_iter()
        .skip(offset)
        .take(limit)
        .filter_map(|group| project_group(&lib, group, kind))
        .collect();
    DuplicateScanResponse {
        groups,
        total,
        offset: req.offset.max(0),
        limit: req.limit.clamp(1, 100),
    }
}

fn project_group(
    lib: &crate::api::LibraryState,
    group: CoreGroup,
    kind: &str,
) -> Option<DuplicateGroupDto> {
    let entries: Vec<DuplicateEntryDto> = group
        .members
        .into_iter()
        .filter_map(|member| {
            let item = lib.index.get_dto_at(&member.id, Some(&member.path))?;
            Some(DuplicateEntryDto {
                item,
                similarity_distance: (kind == "similar").then_some(member.distance),
            })
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    Some(DuplicateGroupDto {
        id: group.id,
        kind: kind.to_string(),
        entries,
    })
}

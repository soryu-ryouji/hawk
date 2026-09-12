//! item 查询端点：list/skeleton/detail/count。list 与 skeleton 走同一条 build_query，次序逐位一致。

use super::*;
use crate::core::color_math;
use crate::core::locks::LockGuard;
use axum::extract::Extension;

// ---------- list / skeleton / detail / count ----------

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", default)]
pub(crate) struct ItemListRequest {
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
    exclude_folders: Option<Vec<String>>,
    without_categories: bool,
    without_tags: bool,
    ext: Option<String>,
    annotation: Option<String>,
    url: Option<String>,
    /// 分辨率档位筛选（短边 = min(width, height)，像素）：≥ 阈值，可与 min_side_lte 组成区间
    min_side_gte: Option<i32>,
    /// 分辨率档位筛选（短边 = min(width, height)，像素）：≤ 阈值，可与 min_side_gte 组成区间
    min_side_lte: Option<i32>,
    /// 宽度筛选（像素）：≥ 阈值，可与 max_width 组成区间；与高度条件独立
    min_width: Option<i32>,
    /// 宽度筛选（像素）：≤ 阈值，可与 min_width 组成区间
    max_width: Option<i32>,
    /// 高度筛选（像素）：≥ 阈值，可与 max_height 组成区间；与宽度条件独立
    min_height: Option<i32>,
    /// 高度筛选（像素）：≤ 阈值，可与 min_height 组成区间
    max_height: Option<i32>,
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
            exclude_folders: None,
            without_categories: false,
            without_tags: false,
            ext: None,
            annotation: None,
            url: None,
            min_side_gte: None,
            min_side_lte: None,
            min_width: None,
            max_width: None,
            min_height: None,
            max_height: None,
            color: None,
            in_trash: false,
            order_by: None,
            order: None,
            offset: 0,
            limit: 50,
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemListResponse {
    items: Vec<ItemDto>,
    total: usize,
    total_size: i64,
    offset: i32,
    limit: i32,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemSkeletonResponse {
    items: Vec<ItemSkeletonDto>,
    total_size: i64,
}

/// ItemListRequest → ItemQuery：/list 与 /skeleton 必须走同一条路径,保证两次查询次序逐位一致。
/// 锁强制：主动筛选（folders/categories/tags）命中未解锁的锁 → 403 LOCKED；
/// 未解锁的锁作为独立排除段随 ItemQuery 传递（与 global_filter 的 exclude 参数同语义不同通道）
fn build_query(req: ItemListRequest, guard: &LockGuard) -> Result<ItemQuery, ApiError> {
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
        exclude_folders: req.exclude_folders,
        without_categories: req.without_categories,
        without_tags: req.without_tags,
        ext: req.ext,
        annotation: req.annotation,
        url: req.url,
        min_side_gte: req.min_side_gte,
        min_side_lte: req.min_side_lte,
        min_width: req.min_width,
        max_width: req.max_width,
        min_height: req.min_height,
        max_height: req.max_height,
        color,
        lock_guard: guard.clone(),
        in_trash: req.in_trash,
        order_by: req.order_by,
        order: req.order,
        offset: req.offset,
        limit: req.limit.max(1),
    })
}

/// 分页查询：全过滤条件 AND 组合，主键同值按 id 打破平局，次序与 skeleton 逐位一致
#[utoipa::path(
    post,
    path = "/api/v1/item/list",
    tags = ["item"],
    request_body = ItemListRequest,
    responses(
        (status = 200, description = "OK", body = Envelope<ItemListResponse>),
        (status = 403, description = "视图位于未解锁的锁定文件夹/分类/标签内（LOCKED）")
    )
)]
pub(crate) async fn item_list(
    State(state): State<SharedState>,
    Extension(guard): Extension<LockGuard>,
    JsonBody(req): JsonBody<ItemListRequest>,
) -> Result<Json<Envelope<ItemListResponse>>, ApiError> {
    check_view_locks(&req, &guard)?;
    let query = build_query(req, &guard)?;
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

/// 骨架查询：与 list 同查询同排序，返回轻量骨架（虚拟网格布局依据）
#[utoipa::path(
    post,
    path = "/api/v1/item/skeleton",
    tags = ["item"],
    request_body = ItemListRequest,
    responses(
        (status = 200, description = "OK", body = Envelope<ItemSkeletonResponse>),
        (status = 403, description = "视图位于未解锁的锁定文件夹/分类/标签内（LOCKED）")
    )
)]
pub(crate) async fn item_skeleton(
    State(state): State<SharedState>,
    Extension(guard): Extension<LockGuard>,
    JsonBody(req): JsonBody<ItemListRequest>,
) -> Result<Json<Envelope<ItemSkeletonResponse>>, ApiError> {
    check_view_locks(&req, &guard)?;
    let query = build_query(req, &guard)?;
    let (items, total_size) = state.index.query_skeleton(&query);
    dispatch_dim_heal(&state, items.iter().map(|i| (i.id.as_str(), i.width)));
    Ok(Json(Envelope::ok(ItemSkeletonResponse {
        items,
        total_size,
    })))
}

/// 读取端宽高自愈的判定：0 宽高且未被负缓存标记为「已探测的非可解码图像」。
/// 后者的 0 宽高是终态，重复派发只会让 worker 反复回写负缓存
fn should_heal_dim(width: i32, negative_cached: bool) -> bool {
    width == 0 && !negative_cached
}

/// 读取端宽高自愈：响应中发现 0 × 0 的 item → 派发后台补全任务（identify 补宽高 + 按需调色板）。
/// 入库时解码暂时失败会把 width=0 落库且无事件再触及，用户拉列表即触发重试，
/// 修复后经 item.updated 事件自动刷新骨架/卡片。in-flight 去重，幂等，高频调用零负担
fn dispatch_dim_heal<'a>(state: &SharedState, items: impl Iterator<Item = (&'a str, i32)>) {
    for (hash, width) in items {
        if !should_heal_dim(width, state.store.palette_negative_cached(hash)) {
            continue;
        }
        if let Some(abs) = state.index.main_source_abs(hash, &state.paths) {
            state.worker.enqueue_palette(hash, &abs);
        }
    }
}

/// 主动筛选命中未解锁的锁 → 403 LOCKED（视图本身受锁保护，拒绝为未解锁请求服务）
fn check_view_locks(req: &ItemListRequest, guard: &LockGuard) -> Result<(), ApiError> {
    if guard.is_empty() {
        return Ok(());
    }
    if let Some(folders) = &req.folders {
        if folders.iter().any(|f| guard.folder_view_locked(f)) {
            return Err(ApiError::locked("文件夹已锁定，需要解锁后访问"));
        }
    }
    if let Some(categories) = &req.categories {
        if categories.iter().any(|c| guard.category_locked(c)) {
            return Err(ApiError::locked("分类已锁定，需要解锁后访问"));
        }
    }
    if let Some(tags) = &req.tags {
        if tags.iter().any(|t| guard.tag_locked(t)) {
            return Err(ApiError::locked("标签已锁定，需要解锁后访问"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::should_heal_dim;

    /// 正常图像（宽高已知）：不派发
    #[test]
    fn dim_known_no_dispatch() {
        assert!(!should_heal_dim(1024, false));
        assert!(!should_heal_dim(1024, true));
    }

    /// 宽高缺失且未探测过：派发（临时失败可自愈）
    #[test]
    fn dim_missing_dispatches() {
        assert!(should_heal_dim(0, false));
    }

    /// 宽高缺失但已负缓存（非可解码图像）：0 是终态，不派发
    #[test]
    fn dim_missing_negative_cached_skips() {
        assert!(!should_heal_dim(0, true));
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
pub(crate) struct ItemAggregateRequest {
    ids: Vec<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ItemAggregateResponse {
    /// 全部选中项的标签交集（排序稳定）
    common_tags: Vec<String>,
    /// 全部选中项的分类交集（排序稳定）
    common_categories: Vec<String>,
}

/// 选择集共有特性聚合（标签/分类交集）。多选面板的「共同标签/分类」数据源——
/// 前端详情缓存只覆盖视口窗口，选择集可达数万项，交集只能由服务端全量计算。
/// 任一 id 位于未解锁的锁覆盖内 → 403 LOCKED（正常流程拿不到被锁 id，锁状态变化后
/// 前端会收到 locks.changed 并重查）
#[utoipa::path(
    post,
    path = "/api/v1/item/aggregate",
    tags = ["item"],
    request_body = ItemAggregateRequest,
    responses(
        (status = 200, description = "OK", body = Envelope<ItemAggregateResponse>),
        (status = 403, description = "选择集含未解锁锁覆盖的 item（LOCKED）")
    )
)]
pub(crate) async fn item_aggregate(
    State(state): State<SharedState>,
    Extension(guard): Extension<LockGuard>,
    JsonBody(req): JsonBody<ItemAggregateRequest>,
) -> Result<Json<Envelope<ItemAggregateResponse>>, ApiError> {
    if req.ids.is_empty() {
        return Err(ApiError::invalid_param("ids 不能为空"));
    }
    if !guard.is_empty() {
        for id in &req.ids {
            if let Some((paths, categories, tags)) = state.index.lock_projection(id) {
                if !guard.item_visible(paths.into_iter(), &categories, &tags) {
                    return Err(ApiError::locked("选择集含已锁定的内容，需要解锁后操作"));
                }
            }
        }
    }
    let (common_tags, common_categories) = state.index.common_taxonomy(&req.ids);
    Ok(Json(Envelope::ok(ItemAggregateResponse {
        common_tags,
        common_categories,
    })))
}

/// 单 item 详情（锁内投影）
#[utoipa::path(
    get,
    path = "/api/v1/item/detail",
    tags = ["item"],
    params(IdQuery),
    responses(
        (status = 200, description = "OK", body = Envelope<ItemDto>),
        (status = 403, description = "位置或内容位于未解锁的锁覆盖内（LOCKED）")
    )
)]
pub(crate) async fn item_detail(
    State(state): State<SharedState>,
    Extension(guard): Extension<LockGuard>,
    Query(q): Query<IdQuery>,
) -> Result<Json<Envelope<ItemDto>>, ApiError> {
    let dto = state
        .index
        .get_dto_at(&q.id, q.path.as_deref())
        .ok_or_else(|| ApiError::item_not_found(&q.id))?;
    if !guard.entry_visible(&dto.path, &dto.categories, &dto.tags) {
        return Err(ApiError::locked("内容已锁定，需要解锁后访问"));
    }
    Ok(Json(Envelope::ok(dto)))
}

/// 库内 item 总数（不含回收站）
#[utoipa::path(
    get,
    path = "/api/v1/item/count",
    tags = ["item"],
    responses((status = 200, description = "OK", body = Envelope<usize>))
)]
pub(crate) async fn item_count(State(state): State<SharedState>) -> Json<Envelope<usize>> {
    Json(Envelope::ok(state.index.count()))
}

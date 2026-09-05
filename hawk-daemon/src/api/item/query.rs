//! item 查询端点：list/skeleton/detail/count。list 与 skeleton 走同一条 build_query，次序逐位一致。

use super::*;
use crate::core::color_math;

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
        exclude_folders: req.exclude_folders,
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

/// 分页查询：全过滤条件 AND 组合，主键同值按 id 打破平局，次序与 skeleton 逐位一致
#[utoipa::path(
    post,
    path = "/api/v1/item/list",
    tags = ["item"],
    request_body = ItemListRequest,
    responses((status = 200, description = "OK", body = Envelope<ItemListResponse>))
)]
pub(crate) async fn item_list(
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

/// 骨架查询：与 list 同查询同排序，返回轻量骨架（虚拟网格布局依据）
#[utoipa::path(
    post,
    path = "/api/v1/item/skeleton",
    tags = ["item"],
    request_body = ItemListRequest,
    responses((status = 200, description = "OK", body = Envelope<ItemSkeletonResponse>))
)]
pub(crate) async fn item_skeleton(
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

/// 单 item 详情（锁内投影）
#[utoipa::path(
    get,
    path = "/api/v1/item/detail",
    tags = ["item"],
    params(IdQuery),
    responses((status = 200, description = "OK", body = Envelope<ItemDto>))
)]
pub(crate) async fn item_detail(
    State(state): State<SharedState>,
    Query(q): Query<IdQuery>,
) -> Result<Json<Envelope<ItemDto>>, ApiError> {
    let dto = state
        .index
        .get_dto_at(&q.id, q.path.as_deref())
        .ok_or_else(|| ApiError::item_not_found(&q.id))?;
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

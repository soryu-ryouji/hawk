//! SSE 订阅素材库变更。EventSource 无法设置请求头，token 经查询参数传递（鉴权中间件放行）。
//! 消费跟不上(积压 1024 条)时服务端直接断开该订阅——客户端重连后必须以
//! item/skeleton + folder/list 全量对齐

use crate::api::SharedState;
use axum::extract::State;
use axum::response::Response;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<SharedState> {
    OpenApiRouter::new().routes(routes!(events))
}

/// SSE 事件订阅：素材库变更推送。帧格式 `event: <事件名>` + `data: <JSON 载荷>`；
/// 全部事件名与载荷结构见 SseEvents（键即事件名）。
/// 消费跟不上（lagged）或总线关闭即断开；重连后须以 item/skeleton + folder/list 全量对齐
#[utoipa::path(
    get,
    path = "/api/v1/events",
    tags = ["events"],
    params(("token" = String, Query, description = "访问 token（EventSource 无法设置请求头）")),
    responses((status = 200, description = "text/event-stream 长连接", content_type = "text/event-stream", body = String))
)]
async fn events(State(state): State<SharedState>) -> Response {
    let mut rx = state.bus.subscribe();
    // lagged（消费跟不上）/总线关闭 → 结束流（断开订阅，客户端重连全量对齐）
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let frame = axum::body::Bytes::from(format!(
                        "event: {}\ndata: {}\n\n",
                        event.kind,
                        serde_json::to_string(&event.payload).unwrap()
                    ));
                    yield Ok::<axum::body::Bytes, std::convert::Infallible>(frame);
                }
                Err(_) => break,
            }
        }
    };
    Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
}

// ---------- SSE 事件载荷注册表（OpenAPI components，与 ItemEvents 常量一一对应） ----------

use crate::core::events::TaskProgress;
use crate::core::item::ItemDto;

/// SSE 事件载荷注册表：键为 `event:` 帧的事件名，值为 `data:` 帧 JSON 载荷的结构。
/// 与 core::taxonomy::ItemEvents 常量一一对应（契约测试双向比对）
#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
pub struct SseEvents {
    #[serde(rename = "item.added")]
    item_added: ItemDto,
    /// 批量入库（扫描导入）合并事件，与单条 item.added 互斥；客户端按「有新增」信号重载骨架
    #[serde(rename = "items.added")]
    items_added: ItemsAddedPayload,
    #[serde(rename = "item.updated")]
    item_updated: ItemDto,
    /// item.updated 的批量变体（调色板批量回写等）
    #[serde(rename = "items.updated")]
    items_updated: ItemsUpdatedPayload,
    #[serde(rename = "item.trashed")]
    item_trashed: ItemIdPayload,
    #[serde(rename = "item.restored")]
    item_restored: ItemDto,
    #[serde(rename = "item.removed")]
    item_removed: ItemIdPayload,
    #[serde(rename = "folder.changed")]
    folder_changed: FolderChangedPayload,
    #[serde(rename = "task.progress")]
    task_progress: TaskProgress,
}

#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
pub struct ItemsAddedPayload {
    ids: Vec<String>,
}

#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
pub struct ItemsUpdatedPayload {
    items: Vec<ItemDto>,
}

#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
pub struct ItemIdPayload {
    id: String,
}

/// reason 目前恒为 external，客户端应忽略取值（结构为将来预留）
#[allow(dead_code)]
#[derive(utoipa::ToSchema)]
pub struct FolderChangedPayload {
    reason: String,
}

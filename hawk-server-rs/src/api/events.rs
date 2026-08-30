//! SSE 订阅素材库变更。EventSource 无法设置请求头，token 经查询参数传递（鉴权中间件放行）。
//! 消费跟不上(积压 1024 条)时服务端直接断开该订阅——客户端重连后必须以
//! item/skeleton + folder/list 全量对齐

use crate::api::SharedState;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;

pub fn routes() -> Router<SharedState> {
    Router::new().route("/api/v1/events", get(events))
}

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

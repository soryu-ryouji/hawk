//! OpenAPI schema 服务。schema 由代码生成（utoipa，路由与文档同一来源），
//! 固化于 openapi.json（`--dump-openapi` 重新生成，契约测试校验同步），
//! 前后端类型从它生成。

use std::sync::OnceLock;
use utoipa::{PartialSchema, ToSchema};

/// SSE 事件载荷等无端点引用的 schema：手工注册进 components（SseEvents 的键即事件名，
/// 契约测试与 core::taxonomy::ItemEvents 常量双向比对）
pub fn attach_extra_schemas(doc: &mut utoipa::openapi::OpenApi) {
    let components = doc.components.get_or_insert_with(Default::default);
    for (name, schema) in [
        (crate::api::events::SseEvents::name(), crate::api::events::SseEvents::schema()),
        (crate::api::events::ItemsAddedPayload::name(), crate::api::events::ItemsAddedPayload::schema()),
        (crate::api::events::ItemsUpdatedPayload::name(), crate::api::events::ItemsUpdatedPayload::schema()),
        (crate::api::events::ItemIdPayload::name(), crate::api::events::ItemIdPayload::schema()),
        (crate::api::events::FolderChangedPayload::name(), crate::api::events::FolderChangedPayload::schema()),
        (crate::core::events::TaskProgress::name(), crate::core::events::TaskProgress::schema()),
    ] {
        components.schemas.insert(name.into_owned(), schema);
    }
}

pub async fn openapi_schema() -> axum::response::Response {
    static CACHED: OnceLock<String> = OnceLock::new();
    let json = CACHED.get_or_init(super::build_openapi_json);
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(json.clone()))
        .unwrap()
}

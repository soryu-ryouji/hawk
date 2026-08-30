//! OpenAPI schema 服务。schema 即契约（前后端类型从它生成），固化自 C# 版输出，
//! 内容不随后端实现漂移

pub async fn openapi_schema() -> axum::response::Response {
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(include_str!("../../openapi.json")))
        .unwrap()
}

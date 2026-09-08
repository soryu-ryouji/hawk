//! 库级集成测试：验证 lib 可被外部依赖（拆 lib 的目的），并固化最基础的契约不变量。
//! 详细的 OpenAPI 契约校验仍在 src/api/contract_tests.rs（可访问 crate 内部）。

/// 生成器可独立构建，且核心端点在列
#[test]
fn openapi_document_builds_and_contains_core_routes() {
    let raw = hawk_daemon::api::build_openapi_json();
    let doc: serde_json::Value = serde_json::from_str(&raw).expect("openapi 应为合法 JSON");
    let paths = doc["paths"].as_object().expect("paths 对象");
    for path in [
        "/api/v1/app/startup",
        "/api/v1/item/list",
        "/api/v1/item/skeleton",
        "/api/v1/folder/list",
        "/api/v1/events",
    ] {
        assert!(paths.contains_key(path), "缺少端点 {path}");
    }
}

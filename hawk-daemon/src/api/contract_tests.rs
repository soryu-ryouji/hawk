//! OpenAPI 契约校验：代码生成（utoipa）的 schema 与路由实现、固化产物的一致性。
//!
//! 1. 同步性：固化的 openapi.json 与 `build_openapi_json()` 语义相等
//!    ——改 API 后须重新固化：`cargo run -- --dump-openapi > openapi.json`
//! 2. 完备性：schema 声明的每个 path+method 必须恰好归入 SUCCESS_CASES（成功路径校验）/
//!    WRITE_SCRIPT（写端点剧本校验）/ ROUTE_ONLY（仅路由存在）/ SSE_ENDPOINTS（SSE 不发 HTTP 请求）
//!    之一——新增端点未归类即测试失败，防止端点静默游离于契约校验之外
//! 3. 响应结构：成功路径真实调用返回 200，响应体经 jsonschema 校验
//!    （$ref 经 components 提升为文档根解析，FolderNode 自引用等循环引用可处理）
//! 4. SSE：事件名集合与 SseEvents schema 双向比对；item.added 经真实订阅捕获校验载荷

use super::lan::LanSupervisor;
use super::{build_router, AppState, SharedState};
use crate::core::config::LibraryConfig;
use crate::core::events::{EventBus, LibraryEvents};
use crate::core::index::ItemIndex;
use crate::core::index_db::IndexDb;
use crate::core::metadata_store::MetadataStore;
use crate::core::paths::LibraryPaths;
use crate::core::pipeline::IndexPipeline;
use crate::core::scanner::LibraryScanner;
use crate::core::startup::StartupState;
use crate::core::taxonomy::{CategoryRegistry, ItemEvents, TagRegistry, TaxonomyMigrator};
use crate::core::thumbnail::ThumbnailService;
use crate::core::thumbnail_worker::ThumbnailWorker;
use crate::core::view_prefs::ViewPreferences;
use crate::settings::Settings;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "contract-test-token";

/// 空库上可直接成功调用的端点：(method, uri, json_body)。
/// 响应体若声明了 200 schema 则校验结构
const SUCCESS_CASES: &[(&str, &str, Option<&str>)] = &[
    ("GET", "/health", None),
    ("GET", "/api/v1/app/info", None),
    ("GET", "/api/v1/app/lan", None),
    ("GET", "/api/v1/app/startup", None),
    ("GET", "/api/v1/app/status", None),
    ("GET", "/api/v1/app/token", None),
    ("GET", "/api/v1/category/list", None),
    ("GET", "/api/v1/folder/list", None),
    ("GET", "/api/v1/global_filter/list", None),
    (
        "PUT",
        "/api/v1/global_filter",
        Some(r#"{"kind":"category","name":"契约隐藏","hidden":true}"#),
    ),
    ("GET", "/api/v1/item/count", None),
    ("GET", "/api/v1/library/info", None),
    ("PATCH", "/api/v1/library/info", Some(r#"{"name":"契约测试库"}"#)),
    ("GET", "/api/v1/tag/list", None),
    ("GET", "/api/v1/view/preferences", None),
    ("POST", "/api/v1/item/list", Some("{}")),
    ("POST", "/api/v1/item/skeleton", Some("{}")),
    ("POST", "/api/v1/library/reindex", None),
    ("POST", "/api/v1/library/rescan", None),
    ("POST", "/api/v1/library/refresh_cache", Some(r#"{"type":"library"}"#)),
    ("POST", "/api/v1/trash/clear", None),
    (
        "PUT",
        "/api/v1/view/preference",
        Some(r#"{"scope":"folder:","order_by":"modification_time","order":"desc"}"#),
    ),
    ("DELETE", "/api/v1/view/preference?scope=folder%3A", None),
];

/// 写端点成功路径剧本覆盖的端点（详见 write_endpoints_match_schema）
const WRITE_SCRIPT: &[(&str, &str)] = &[
    ("GET", "/api/v1/item/detail"),
    ("GET", "/api/v1/item/file"),
    ("GET", "/api/v1/item/thumbnail"),
    ("POST", "/api/v1/item/update"),
    ("POST", "/api/v1/item/batch_update"),
    ("POST", "/api/v1/item/refresh_thumbnail"),
    ("POST", "/api/v1/item/replace"),
    ("POST", "/api/v1/item/delete"),
    ("POST", "/api/v1/item/restore"),
    ("POST", "/api/v1/item/add"),
    ("POST", "/api/v1/item/upload"),
    ("POST", "/api/v1/category/create"),
    ("POST", "/api/v1/category/update"),
    ("POST", "/api/v1/category/delete"),
    ("POST", "/api/v1/tag/create"),
    ("POST", "/api/v1/tag/update"),
    ("POST", "/api/v1/tag/delete"),
    ("POST", "/api/v1/folder/create"),
    ("POST", "/api/v1/folder/update"),
    ("POST", "/api/v1/folder/delete"),
    ("POST", "/api/v1/folder/restore"),
];

/// 只校验路由存在的端点：app/lan PUT 需运行中的 LAN supervisor 收敛，不在测试内成功调用
const ROUTE_ONLY: &[(&str, &str)] = &[("PUT", "/api/v1/app/lan")];

/// SSE 长连接端点：不发 HTTP 请求（oneshot 会挂起），只做 schema 存在性与事件集合校验
const SSE_ENDPOINTS: &[(&str, &str)] = &[("GET", "/api/v1/events")];

fn spec() -> Value {
    serde_json::from_str(include_str!("../../openapi.json")).expect("openapi.json 解析失败")
}

/// 测试装配：与 main.rs 同一套组件接线（无 watcher / LAN 监听 / 周期对账），
/// 目录落在系统临时目录（用毕即删）
struct TestApp {
    base: PathBuf,
    router: axum::Router,
    state: SharedState,
}

fn test_app(name: &str) -> TestApp {
    let base = std::env::temp_dir().join(format!("hawk-contract-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("library");
    let cache = base.join("cache");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&cache).unwrap();

    let root_str = root.to_string_lossy().to_string();
    let paths = LibraryPaths::new(&root_str, Some(cache.to_string_lossy().to_string()));
    paths.ensure_layout();
    let config = Arc::new(LibraryConfig::new(paths.clone()));
    let db = Arc::new(IndexDb::open(&paths.index_db_file));
    let startup = Arc::new(StartupState::default());
    let store = Arc::new(MetadataStore::new(paths.clone(), db.clone(), &startup));
    let index = Arc::new(ItemIndex::default());
    let bus = EventBus::new();
    let categories = Arc::new(CategoryRegistry::new(&paths));
    let tags = Arc::new(TagRegistry::new(&paths));
    let prefs = Arc::new(ViewPreferences::new(&paths));
    let global_filter = Arc::new(crate::core::global_filter::GlobalFilter::new(&paths));
    let thumbs = ThumbnailService::new(Arc::new(paths.clone()));
    let worker = ThumbnailWorker::new(thumbs.clone(), bus.clone());
    let migrator = Arc::new(TaxonomyMigrator::new(
        store.clone(),
        index.clone(),
        categories.clone(),
        tags.clone(),
        bus.clone(),
    ));
    let scanner = LibraryScanner::new(paths.clone(), config.clone());
    let settings = Settings {
        library_root: root_str,
        port: 0,
        token: TOKEN.to_string(),
        rescan_interval_seconds: 0, // 关闭周期对账：测试只断言显式驱动的行为
        cache_parent: None,
        web_dist: None,
    };
    let pipeline = IndexPipeline::new(
        paths.clone(),
        config.clone(),
        store.clone(),
        index.clone(),
        thumbs.clone(),
        bus.clone(),
        scanner,
        migrator,
        prefs.clone(),
        global_filter.clone(),
        worker.clone(),
        startup.clone(),
        settings.clone(),
    );
    worker.attach(index.clone(), store.clone(), pipeline.sender());
    pipeline.start();
    startup.mark_ready();

    let state: SharedState = Arc::new(AppState {
        settings,
        paths,
        config,
        startup,
        index,
        bus,
        pipeline,
        thumbs,
        prefs,
        categories,
        tags,
        global_filter,
        worker,
        lan: LanSupervisor::new(),
    });
    let router = build_router(state.clone());
    TestApp { base, router, state }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

impl TestApp {
    fn library_root(&self) -> PathBuf {
        self.base.join("library")
    }

    /// 写入 8×8 纯色 PNG 并经流水线入库（mtime 拨回一天避开写入防抖），返回 item id
    async fn add_test_item(&self, rel: &str, rgb: [u8; 3]) -> String {
        let abs = self.library_root().join(rel);
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb(rgb));
        img.save(&abs).unwrap();
        let day_ago = filetime::FileTime::from_unix_time(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                - 86400,
            0,
        );
        filetime::set_file_mtime(&abs, day_ago).unwrap();

        let result = self
            .state
            .pipeline
            .submit_upsert(abs.to_string_lossy().to_string(), None)
            .await
            .expect("upsert 失败")
            .expect("upsert 被防抖延迟（mtime 已拨回，不应发生）");
        result.item.id
    }
}

/// 发起请求（携带 admin token 与环回 Host 头），返回状态码与响应体字节
async fn call(router: &axum::Router, method: &str, uri: &str, body: Option<(&str, Vec<u8>)>) -> (StatusCode, Vec<u8>) {
    let (content_type, bytes) = match body {
        Some((ct, b)) => (ct.to_string(), b),
        None => ("application/json".to_string(), Vec::new()),
    };
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("host", "127.0.0.1")
        .header("content-type", content_type)
        .body(Body::from(bytes))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    (status, bytes.to_vec())
}

async fn call_json(router: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Vec<u8>) {
    call(
        router,
        method,
        uri,
        body.map(|b| ("application/json", serde_json::to_vec(&b).unwrap())),
    )
    .await
}

/// 取端点 200 响应的 JSON schema，包装为自包含文档（components 提升为根，
/// 使 `#/components/schemas/...` 引用——含 FolderNode 式循环引用——可解析）
fn response_schema(spec: &Value, method: &str, path: &str) -> Option<Value> {
    let pointer = format!(
        "/paths/{}/{}/responses/200/content/application~1json/schema",
        path.replace('/', "~1"),
        method.to_lowercase()
    );
    spec.pointer(&pointer).map(|s| {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "components": spec["components"].clone(),
            "allOf": [s.clone()],
        })
    })
}

/// 断言 200 且响应体符合端点 200 schema（无 application/json content 的端点只验 200）
async fn expect_ok(app: &TestApp, spec: &Value, method: &str, uri: &str, body: Option<Value>) {
    let (status, bytes) = call_json(&app.router, method, uri, body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{method} {uri} 期望 200，实际 {status}，响应: {}",
        String::from_utf8_lossy(&bytes)
    );

    let path = uri.split('?').next().unwrap();
    let Some(wrapper) = response_schema(spec, method, path) else {
        return; // 二进制/文本端点无 JSON schema，仅验 200
    };
    let validator = jsonschema::validator_for(&wrapper)
        .unwrap_or_else(|e| panic!("schema 编译失败 {method} {path}: {e}"));
    let body_json: Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("响应非 JSON {method} {uri}: {e}"));
    let errors: Vec<String> = validator.iter_errors(&body_json).map(|e| e.to_string()).collect();
    assert!(
        errors.is_empty(),
        "响应不符合契约 {method} {uri}:\n{}\n响应体: {body_json}",
        errors.join("\n")
    );
}

/// 固化产物同步：openapi.json 与代码生成语义相等（Value 比较，免疫键序/空白差异）
#[test]
fn openapi_json_in_sync() {
    let generated: Value = serde_json::from_str(&super::build_openapi_json()).unwrap();
    let fixed = spec();
    assert!(
        generated == fixed,
        "openapi.json 与代码生成不同步，请重新固化：cargo run -- --dump-openapi > openapi.json"
    );
}

/// 完备性：openapi.json 声明的端点集必须恰好等于四类归类的并集
#[test]
fn openapi_endpoints_all_classified() {
    let spec = spec();
    let mut declared = BTreeSet::new();
    for (path, item) in spec["paths"].as_object().unwrap() {
        for method in ["get", "post", "put", "patch", "delete"] {
            if item.get(method).is_some() {
                declared.insert((method.to_uppercase(), path.clone()));
            }
        }
    }

    let mut covered = BTreeSet::new();
    for group in [
        SUCCESS_CASES.iter().map(|(m, u, _)| (*m, u.split('?').next().unwrap())).collect::<Vec<_>>(),
        WRITE_SCRIPT.to_vec(),
        ROUTE_ONLY.to_vec(),
        SSE_ENDPOINTS.to_vec(),
    ] {
        for (method, path) in group {
            assert!(
                covered.insert((method.to_string(), path.to_string())),
                "端点重复归类: {method} {path}"
            );
        }
    }

    let missing: Vec<_> = declared.difference(&covered).collect();
    let extra: Vec<_> = covered.difference(&declared).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "契约测试归类与 openapi.json 不一致。\n未归类（请加入 SUCCESS_CASES / WRITE_SCRIPT / ROUTE_ONLY / SSE_ENDPOINTS）: {missing:?}\n清单多余（端点已从 schema 移除）: {extra:?}"
    );
}

/// 成功路径：空库真实调用返回 200，响应体符合 200 schema
#[tokio::test]
async fn success_cases_match_schema() {
    let app = test_app("success");
    let spec = spec();
    for (method, uri, body) in SUCCESS_CASES {
        let body = body.map(|s| serde_json::from_str(s).unwrap());
        expect_ok(&app, &spec, method, uri, body).await;
    }
}

/// 写端点剧本：准备真实 item 后按依赖顺序调用全部写端点，校验 200 与响应 schema
#[tokio::test]
async fn write_endpoints_match_schema() {
    let app = test_app("write");
    let spec = spec();
    let id = app.add_test_item("a.png", [200, 30, 30]).await;

    // 读：详情 / 原图 / 缩略图（回源原图）
    expect_ok(&app, &spec, "GET", &format!("/api/v1/item/detail?id={id}"), None).await;
    let (status, bytes) = call(&app.router, "GET", &format!("/api/v1/item/file?id={id}"), None).await;
    assert_eq!(status, StatusCode::OK, "item/file");
    assert!(!bytes.is_empty(), "item/file 响应为空");
    let (status, bytes) = call(&app.router, "GET", &format!("/api/v1/item/thumbnail?id={id}"), None).await;
    assert_eq!(status, StatusCode::OK, "item/thumbnail");
    assert!(!bytes.is_empty(), "item/thumbnail 响应为空");

    // 元数据写
    expect_ok(&app, &spec, "POST", "/api/v1/item/update", Some(json!({"id": id, "star": 5, "tags": ["契约标记"]}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/item/batch_update", Some(json!({"ids": [id], "add_categories": ["契约分类"]}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/item/refresh_thumbnail", Some(json!({"id": id}))).await;

    // 内容替换：同内容幂等（哈希不变，直接返回当前投影）
    let same_bytes = std::fs::read(app.library_root().join("a.png")).unwrap();
    let same_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &same_bytes);
    expect_ok(&app, &spec, "POST", "/api/v1/item/replace", Some(json!({"id": id, "img_base64": same_b64}))).await;

    // 分类 / 标签生命周期
    expect_ok(&app, &spec, "POST", "/api/v1/category/create", Some(json!({"name": "剧本分类"}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/category/update", Some(json!({"name": "剧本分类", "new_name": "剧本分类2"}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/category/delete", Some(json!({"name": "剧本分类2"}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/tag/create", Some(json!({"name": "剧本标签"}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/tag/update", Some(json!({"name": "剧本标签", "new_name": "剧本标签2"}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/tag/delete", Some(json!({"name": "剧本标签2"}))).await;

    // 文件夹生命周期（delete 入回收站后 restore 放回）
    expect_ok(&app, &spec, "POST", "/api/v1/folder/create", Some(json!({"name": "剧本目录"}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/folder/update", Some(json!({"path": "剧本目录", "name": "剧本目录2"}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/folder/delete", Some(json!({"path": "剧本目录2"}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/folder/restore", Some(json!({"path": "剧本目录2"}))).await;

    // 回收站往返
    expect_ok(&app, &spec, "POST", "/api/v1/item/delete", Some(json!({"id": id}))).await;
    expect_ok(&app, &spec, "POST", "/api/v1/item/restore", Some(json!({"id": id}))).await;

    // 入库两通道：base64 导入新内容 + multipart 上传
    let png_b = {
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([30, 200, 30]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    };
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_b);
    expect_ok(&app, &spec, "POST", "/api/v1/item/add", Some(json!({"img_base64": b64, "name": "b"}))).await;

    let boundary = "contract-boundary";
    let mut multipart = Vec::new();
    multipart.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"c.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
    multipart.extend_from_slice(&png_b);
    multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let (status, bytes) = call(
        &app.router,
        "POST",
        "/api/v1/item/upload",
        Some(("multipart/form-data; boundary=contract-boundary", multipart)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "item/upload 期望 200，实际 {status}，响应: {}",
        String::from_utf8_lossy(&bytes)
    );
    let body_json: Value = serde_json::from_slice(&bytes).unwrap();
    let wrapper = response_schema(&spec, "POST", "/api/v1/item/upload").unwrap();
    let validator = jsonschema::validator_for(&wrapper).unwrap();
    let errors: Vec<String> = validator.iter_errors(&body_json).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "item/upload 响应不符合契约:\n{}", errors.join("\n"));
}

/// 路由存在：ROUTE_ONLY 端点发请求，断言非路由缺失。
/// 缺参数/实体不存在等业务拒绝是 JSON 错误信封；路由缺失（fallback 空 404）不是
#[tokio::test]
async fn route_only_endpoints_exist() {
    let app = test_app("route");
    for (method, path) in ROUTE_ONLY {
        let body = matches!(*method, "POST" | "PUT" | "DELETE").then_some("{}");
        let (status, bytes) = call(&app.router, method, path, body.map(str::as_bytes).map(|b| ("application/json", b.to_vec()))).await;
        if status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED {
            let is_error_envelope = serde_json::from_slice::<Value>(&bytes)
                .map(|v| v["status"] == "error")
                .unwrap_or(false);
            assert!(
                is_error_envelope,
                "openapi.json 声明但路由缺失: {method} {path}（{status} 空响应）"
            );
        }
    }
}

/// SSE 契约：事件名集合与 SseEvents schema 双向比对；item.added 真实订阅捕获校验载荷
#[tokio::test]
async fn sse_events_match_schema() {
    let spec = spec();
    let declared: BTreeSet<String> = spec["components"]["schemas"]["SseEvents"]["properties"]
        .as_object()
        .expect("SseEvents schema 缺失")
        .keys()
        .cloned()
        .collect();
    let implemented: BTreeSet<String> = [
        ItemEvents::ADDED,
        ItemEvents::ITEMS_ADDED,
        ItemEvents::UPDATED,
        ItemEvents::ITEMS_UPDATED,
        ItemEvents::TRASHED,
        ItemEvents::RESTORED,
        ItemEvents::REMOVED,
        ItemEvents::FOLDER_CHANGED,
        ItemEvents::TASK_PROGRESS,
        LibraryEvents::UPDATED,
        crate::core::global_filter::GLOBAL_FILTER_CHANGED,
    ]
    .map(str::to_string)
    .into_iter()
    .collect();
    assert_eq!(
        declared,
        implemented,
        "SseEvents schema 与 ItemEvents 常量不一致（增删事件请同步两侧）"
    );

    // 真实捕获：订阅总线 → 入库新文件 → item.added 载荷过 schema
    let app = test_app("sse");
    let mut rx = app.state.bus.subscribe();
    app.add_test_item("sse.png", [30, 30, 200]).await;
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(e) if e.kind == ItemEvents::ADDED => break e,
                Ok(_) => continue, // task.progress 等跳过
                Err(e) => panic!("事件总线错误: {e}"),
            }
        }
    })
    .await
    .expect("5s 内未收到 item.added");

    let wrapper = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "components": spec["components"].clone(),
        "allOf": [spec["components"]["schemas"]["SseEvents"]["properties"]["item.added"].clone()],
    });
    let validator = jsonschema::validator_for(&wrapper).unwrap();
    let errors: Vec<String> = validator.iter_errors(&event.payload).map(|e| e.to_string()).collect();
    assert!(
        errors.is_empty(),
        "item.added 载荷不符合契约:\n{}\n载荷: {}",
        errors.join("\n"),
        event.payload
    );
}

/// 全局列表隐藏：global_filter 端点读写 + item/list 的 exclude_folders/categories/tags 排除语义
#[tokio::test]
async fn global_filter_exclusion() {
    let app = test_app("gfhide");
    // 根目录与子目录各一项；子目录项挂上分类与标签
    let root_id = app.add_test_item("root.png", [10, 10, 10]).await;
    std::fs::create_dir_all(app.library_root().join("积压")).unwrap();
    let sub_id = app.add_test_item("积压/inner.png", [20, 20, 20]).await;
    let (status, _) = call_json(
        &app.router,
        "POST",
        "/api/v1/item/update",
        Some(json!({"id": sub_id, "categories": ["素材堆"], "tags": ["量大"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "item/update 挂分类标签");

    // 端点读写：标记后可从 list 读回；取消后消失
    let (status, _) = call_json(&app.router, "PUT", "/api/v1/global_filter", Some(json!({"kind": "folder", "name": "积压", "hidden": true}))).await;
    assert_eq!(status, StatusCode::OK, "PUT global_filter folder");
    let (_, bytes) = call_json(&app.router, "GET", "/api/v1/global_filter/list", None).await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["folders"], json!(["积压"]), "隐藏文件夹读回");

    // 无排除：两项都在
    let list_total = |body: Value| async {
        let (_, bytes) = call_json(&app.router, "POST", "/api/v1/item/list", Some(body)).await;
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        (v["data"]["total"].as_u64().unwrap(), v["data"].clone())
    };
    let (total, _) = list_total(json!({})).await;
    assert_eq!(total, 2, "无排除时应为全部两项");

    // 文件夹子树排除：只剩根目录项
    let (total, data) = list_total(json!({"exclude_folders": ["积压"]})).await;
    assert_eq!(total, 1, "exclude_folders 子树排除");
    assert_eq!(data["items"][0]["id"], json!(root_id));

    // 分类/标签排除：命中即剔
    let (total, _) = list_total(json!({"exclude_categories": ["素材堆"]})).await;
    assert_eq!(total, 1, "exclude_categories 排除");
    let (total, _) = list_total(json!({"exclude_tags": ["量大"]})).await;
    assert_eq!(total, 1, "exclude_tags 排除");

    // 维度自身视图不受影响：正向 folders 过滤仍能看到子目录项
    let (total, _) = list_total(json!({"folders": ["积压"]})).await;
    assert_eq!(total, 1, "文件夹自身视图不过滤自身");

    // 取消隐藏
    let (status, _) = call_json(&app.router, "PUT", "/api/v1/global_filter", Some(json!({"kind": "folder", "name": "积压", "hidden": false}))).await;
    assert_eq!(status, StatusCode::OK, "PUT global_filter 取消隐藏");
    let (_, bytes) = call_json(&app.router, "GET", "/api/v1/global_filter/list", None).await;
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["folders"], json!([]), "取消后列表为空");

    // 非法参数：空路径 / 非法维度
    let (status, _) = call_json(&app.router, "PUT", "/api/v1/global_filter", Some(json!({"kind": "folder", "name": "", "hidden": true}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "空文件夹路径应 400");
    let (status, _) = call_json(&app.router, "PUT", "/api/v1/global_filter", Some(json!({"kind": "other", "name": "x", "hidden": true}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非法维度应 400");
}

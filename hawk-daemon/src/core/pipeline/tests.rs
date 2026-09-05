//! 索引流水线的行为级测试：单写者消费循环 + 真实临时素材库（文件系统/元数据/索引全链路）。
//! 覆盖三个最易回归的核心不变量：
//! - upsert 幂等（watcher 重发事件/重复提交不制造重复 item 或位置）
//! - 扫描消失对账（停机/监听遗漏的删除由全库扫描收敛）
//! - 移动的同一性继承（改名后 hash 不变、元数据保留——迁移继承是索引设计的基石）
//!
//! 驱动方式：submit_* 系列（携带 oneshot 回复，等待消费循环处理完成），
//! 不断言后台 worker 的提炼结果（缩略图/调色板/宽高为异步补充，与本测试关注的不变量无关）。

use super::*;
use crate::core::index_db::IndexDb;
use crate::core::taxonomy::{CategoryRegistry, ItemEvents, TagRegistry};
use std::path::PathBuf;

/// 测试装配：与 main.rs 同一套组件接线，目录全部落在系统临时目录（用毕即删）
struct Rig {
    root: PathBuf,
    cache: PathBuf,
    pipeline: IndexPipeline,
    index: Arc<ItemIndex>,
    store: Arc<MetadataStore>,
    bus: EventBus,
}

impl Rig {
    fn new(name: &str) -> Rig {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("debug"))
            .with_test_writer()
            .try_init();
        let base = std::env::temp_dir().join(format!("hawk-pipeline-test-{name}-{}", std::process::id()));
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
            categories,
            tags,
            bus.clone(),
        ));
        let scanner = LibraryScanner::new(paths.clone(), config.clone());
        let settings = Settings {
            library_root: root_str,
            port: 0,
            token: "test".to_string(),
            rescan_interval_seconds: 0, // 关闭周期对账：测试只断言显式驱动的行为
            cache_parent: None,
            web_dist: None,
        };
        let pipeline = IndexPipeline::new(
            paths.clone(),
            config,
            store.clone(),
            index.clone(),
            thumbs,
            bus.clone(),
            scanner,
            migrator,
            prefs,
            global_filter,
            worker.clone(),
            startup.clone(),
            settings,
        );
        worker.attach(index.clone(), store.clone(), pipeline.sender());
        pipeline.start();
        startup.mark_ready();

        Rig { root, cache, pipeline, index, store, bus }
    }

    fn abs(&self, rel: &str) -> String {
        self.root.join(rel).to_string_lossy().to_string()
    }

    /// 写一个 8×8 纯色 PNG（真实可解码，worker 后台提炼不会因坏图报错）
    fn write_png(&self, rel: &str, rgb: [u8; 3]) {
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb(rgb));
        img.save(self.root.join(rel)).unwrap();
        self.stabilize(rel);
    }

    /// 复制库内文件（同内容不同位置的场景）
    fn copy_file(&self, from_rel: &str, to_rel: &str) {
        std::fs::copy(self.root.join(from_rel), self.root.join(to_rel)).unwrap();
        self.stabilize(to_rel);
    }

    /// mtime 拨回一天前：upsert 流程对「可能仍在写入」的新文件（mtime 距现在过近）会防抖延迟，
    /// 测试驱动的是确定性的立即处理路径
    fn stabilize(&self, rel: &str) {
        let day_ago = filetime::FileTime::from_unix_time(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                - 86400,
            0,
        );
        filetime::set_file_mtime(self.root.join(rel), day_ago).unwrap();
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(base) = self.root.parent() {
            let _ = std::fs::remove_dir_all(base);
        }
        let _ = &self.cache; // cache 与 root 同 base，随上一步删除
    }
}

#[tokio::test]
async fn upsert_is_idempotent() {
    let rig = Rig::new("upsert-idempotent");
    rig.write_png("a.png", [255, 0, 0]);

    let first = rig.pipeline.submit_upsert(rig.abs("a.png"), None).await.unwrap();
    let first = first.expect("首次 upsert 应入库");
    let id = first.item.id.clone();
    assert_eq!(rig.index.count(), 1);

    // watcher 重发事件（同一路径重复 upsert）：不新增 item、不新增位置
    let again = rig.pipeline.submit_upsert(rig.abs("a.png"), None).await.unwrap();
    assert!(again.is_some());
    assert_eq!(rig.index.count(), 1);
    assert_eq!(rig.index.library_location_count(&id), 1);

    // 同内容复制到另一路径：内容寻址收敛到同一 item，仅多登记一个位置；
    // count 为位置级口径（同内容两位置 = 两个文件）
    rig.copy_file("a.png", "b.png");
    let third = rig.pipeline.submit_upsert(rig.abs("b.png"), None).await.unwrap();
    let third = third.expect("同内容新路径应登记位置");
    assert_eq!(third.item.id, id, "同内容应收敛为同一 item");
    assert_eq!(rig.index.count(), 2);
    assert_eq!(rig.index.library_location_count(&id), 2);
}

/// 同 hash 多位置在查询中展开为独立条目：名称/位置各自，文件夹过滤按位置生效
#[tokio::test]
async fn same_content_locations_expand_in_query() {
    let rig = Rig::new("same-content-expand");
    rig.write_png("dup.png", [255, 0, 0]);
    std::fs::create_dir_all(rig.abs("sub")).unwrap();
    rig.copy_file("dup.png", "sub/dup2.png");
    rig.pipeline.run_scan(false).await.unwrap();

    // 全部视图：两个位置各成一条，名称各自
    let base_q = || crate::core::item::ItemQuery { limit: 50, ..Default::default() };
    let (items, total, _) = rig.index.query(&base_q());
    assert_eq!(total, 2);
    let mut names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["dup", "dup2"]);
    let mut paths: Vec<&str> = items.iter().map(|i| i.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, ["dup.png", "sub/dup2.png"]);

    // 骨架与 list 同序同数（含 path 维度）
    let (skeleton, _) = rig.index.query_skeleton(&base_q());
    assert_eq!(skeleton.len(), 2);
    assert!(skeleton.iter().all(|s| !s.path.is_empty()));

    // 文件夹视图只含该目录内的位置（同内容在别处的位置不出现）
    let q = crate::core::item::ItemQuery {
        folders: Some(vec!["sub".to_string()]),
        folders_exact: true,
        limit: 50,
        ..Default::default()
    };
    let (sub_items, sub_total, _) = rig.index.query(&q);
    assert_eq!(sub_total, 1);
    assert_eq!(sub_items[0].name, "dup2");

    // 按名称过滤按位置生效
    let q = crate::core::item::ItemQuery {
        keywords: Some(vec!["dup2".to_string()]),
        limit: 50,
        ..Default::default()
    };
    let (_, kw_total, _) = rig.index.query(&q);
    assert_eq!(kw_total, 1);

    // 删除一个位置后只剩另一条
    let source = rig.abs("sub/dup2.png");
    let trash = rig.abs(".hawk/trash/sub/dup2.png");
    std::fs::create_dir_all(std::path::Path::new(&trash).parent().unwrap()).unwrap();
    std::fs::rename(&source, &trash).unwrap();
    rig.pipeline.submit_move(source, trash).await.unwrap();
    let (after, after_total, _) = rig.index.query(&base_q());
    assert_eq!(after_total, 1);
    assert_eq!(after[0].name, "dup");
}

#[tokio::test]
async fn scan_reconcile_removes_disappeared() {
    let rig = Rig::new("scan-reconcile");
    rig.write_png("a.png", [255, 0, 0]);
    rig.write_png("b.png", [0, 255, 0]);

    rig.pipeline.run_scan(false).await.unwrap();
    assert_eq!(rig.index.count(), 2);

    // 停机期间的删除（watcher 未观测到）由全库扫描的消失对账收敛
    std::fs::remove_file(rig.abs("a.png")).unwrap();
    rig.pipeline.run_scan(false).await.unwrap();
    assert_eq!(rig.index.count(), 1);

    let remaining = rig.index.all_location_paths();
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].ends_with("b.png"));
}

#[tokio::test]
async fn move_preserves_identity_and_metadata() {
    let rig = Rig::new("move-inherit");
    rig.write_png("a.png", [0, 0, 255]);

    let res = rig.pipeline.submit_upsert(rig.abs("a.png"), None).await.unwrap();
    let id = res.expect("upsert 应入库").item.id;

    // 写入元数据（标签），随后改名——移动必须继承同一性与元数据
    rig.pipeline
        .submit_metadata(id.clone(), |m| m.tags = vec!["风景".to_string()])
        .await
        .unwrap();
    std::fs::rename(rig.abs("a.png"), rig.abs("b.png")).unwrap();
    rig.pipeline.submit_move(rig.abs("a.png"), rig.abs("b.png")).await.unwrap();

    // hash 不变、标签保留、位置已切换到新路径（库内相对路径）
    assert!(rig.index.contains(&id));
    let meta = rig.store.try_get(&id).expect("元数据应保留");
    assert_eq!(meta.tags, vec!["风景".to_string()]);
    let loc = rig.index.find_location(&id, None, None).expect("位置应存在");
    assert_eq!(loc.path, "b.png");
    assert!(rig.index.hash_by_location("b.png").is_some());
    assert!(rig.index.hash_by_location("a.png").is_none());
}

/// 移动的目标路径已存在索引记录时（同内容另一副本）：位置合并而非互相覆盖
#[tokio::test]
async fn move_onto_existing_content_merges_locations() {
    let rig = Rig::new("move-merge");
    rig.write_png("a.png", [255, 255, 0]);
    rig.copy_file("a.png", "b.png");

    let first = rig.pipeline.submit_upsert(rig.abs("a.png"), None).await.unwrap();
    let id = first.expect("upsert 应入库").item.id;
    rig.pipeline.submit_upsert(rig.abs("b.png"), None).await.unwrap();
    assert_eq!(rig.index.library_location_count(&id), 2);

    // a.png 改名为 c.png：位置从 a 切到 c，b 位置不受影响
    std::fs::rename(rig.abs("a.png"), rig.abs("c.png")).unwrap();
    rig.pipeline.submit_move(rig.abs("a.png"), rig.abs("c.png")).await.unwrap();
    assert_eq!(rig.index.library_location_count(&id), 2);
    assert!(rig.index.hash_by_location("c.png").is_some());
    assert!(rig.index.hash_by_location("b.png").is_some());
    assert!(rig.index.hash_by_location("a.png").is_none());
}

/// 目录删除广播 folder.changed：含内容的目录经 Delete 事件删除后客户端据此重拉文件夹树；
/// 纯文件删除不广播（无目录结构变化）
#[tokio::test]
async fn delete_dir_publishes_folder_changed() {
    let rig = Rig::new("delete-dir-fcevent");
    rig.write_png("f.png", [9, 9, 9]);
    rig.pipeline.submit_upsert(rig.abs("f.png"), None).await.unwrap();
    std::fs::create_dir_all(rig.abs("d")).unwrap();
    rig.write_png("d/a.png", [1, 2, 3]);
    rig.pipeline.submit_upsert(rig.abs("d/a.png"), None).await.unwrap();

    let mut rx = rig.bus.subscribe();
    // 纯文件删除：无 folder.changed（随后的 upsert 屏障保证 Delete 已处理完）
    rig.pipeline.notify_deleted(rig.abs("f.png"));
    rig.pipeline.submit_upsert(rig.abs("f.png"), None).await.unwrap();
    let mut file_delete_fired = false;
    loop {
        match rx.try_recv() {
            Ok(e) if e.kind == ItemEvents::FOLDER_CHANGED => file_delete_fired = true,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(!file_delete_fired, "纯文件删除不应广播 folder.changed");

    // 含内容的目录删除：广播 folder.changed
    rig.pipeline.notify_deleted(rig.abs("d"));
    let got = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(e) if e.kind == ItemEvents::FOLDER_CHANGED => break true,
                Ok(_) => continue,
                Err(_) => break false,
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(got, "删除含内容的目录应广播 folder.changed");
    assert_eq!(rig.index.count(), 1, "目录下位置应已清除（f.png 保留）");
}

/// 批量元数据应用：空操作（标签已存在）跳过落盘/事件且不计入 updated；
/// 有变更的项合并为 items.updated 事件（批量不逐条发 item.updated）
#[tokio::test]
async fn batch_metadata_skips_noop_and_batches_events() {
    let rig = Rig::new("batch-noop");
    rig.write_png("a.png", [5, 5, 5]);
    rig.write_png("b.png", [6, 6, 6]);
    let id_a = rig.pipeline.submit_upsert(rig.abs("a.png"), None).await.unwrap().unwrap().item.id;
    let id_b = rig.pipeline.submit_upsert(rig.abs("b.png"), None).await.unwrap().unwrap().item.id;

    // 先给 a 打上标签（b 无标签）
    rig.pipeline
        .submit_metadata(id_a.clone(), |m| m.tags.push("批量".to_string()))
        .await
        .unwrap();

    let mut rx = rig.bus.subscribe();
    let result = rig
        .pipeline
        .submit_batch_metadata(vec![id_a.clone(), id_b.clone()], move |m| {
            if !m.tags.iter().any(|t| t == "批量") {
                m.tags.push("批量".to_string());
            }
        })
        .await
        .unwrap();
    assert_eq!(result.updated, 1, "a 已含标签为空操作，只有 b 实际更新");
    assert!(result.missing_ids.is_empty());

    // 事件：恰有一个 items.updated 帧且只含 b；无 item.updated
    let mut items_updated_frames = 0;
    let mut single_updated = false;
    let mut frame_ids: Vec<String> = Vec::new();
    while let Ok(e) = rx.try_recv() {
        match e.kind {
            ItemEvents::ITEMS_UPDATED => {
                items_updated_frames += 1;
                for item in e.payload["items"].as_array().unwrap() {
                    frame_ids.push(item["id"].as_str().unwrap().to_string());
                }
            }
            ItemEvents::UPDATED => single_updated = true,
            _ => {}
        }
    }
    assert_eq!(items_updated_frames, 1, "应合并为一个 items.updated 帧");
    assert!(!single_updated, "批量路径不应发逐条 item.updated");
    assert_eq!(frame_ids, vec![id_b]);
}

/// 标签删除级联：全部命中 item 的变更合并发 items.updated（不逐条发 item.updated），
/// 事件按流水线处理节奏分块（攒满即flush，长批量渐进可见）
#[tokio::test]
async fn tag_delete_cascade_batches_events() {
    let rig = Rig::new("tag-cascade-batch");
    rig.write_png("a.png", [7, 7, 7]);
    rig.write_png("b.png", [8, 8, 8]);
    let id_a = rig.pipeline.submit_upsert(rig.abs("a.png"), None).await.unwrap().unwrap().item.id;
    let id_b = rig.pipeline.submit_upsert(rig.abs("b.png"), None).await.unwrap().unwrap().item.id;
    rig.pipeline
        .submit_batch_metadata(vec![id_a.clone(), id_b.clone()], |m| m.tags.push("待删".to_string()))
        .await
        .unwrap();

    let mut rx = rig.bus.subscribe();
    rig.pipeline.submit_tag_delete("待删".to_string()).await.unwrap();

    let mut items_updated_ids: Vec<String> = Vec::new();
    let mut single_updated = false;
    while let Ok(e) = rx.try_recv() {
        match e.kind {
            ItemEvents::ITEMS_UPDATED => {
                for item in e.payload["items"].as_array().unwrap() {
                    items_updated_ids.push(item["id"].as_str().unwrap().to_string());
                }
            }
            ItemEvents::UPDATED => single_updated = true,
            _ => {}
        }
    }
    assert!(!single_updated, "级联路径不应发逐条 item.updated");
    items_updated_ids.sort();
    let mut expected = vec![id_a, id_b];
    expected.sort();
    assert_eq!(items_updated_ids, expected, "两个命中项应合并进 items.updated");

    // 元数据与索引已清除该标签
    assert!(rig.store.try_get(&items_updated_ids[0]).unwrap().tags.is_empty());
}

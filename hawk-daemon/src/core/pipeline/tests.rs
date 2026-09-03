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
use crate::core::taxonomy::{CategoryRegistry, TagRegistry};
use std::path::PathBuf;

/// 测试装配：与 main.rs 同一套组件接线，目录全部落在系统临时目录（用毕即删）
struct Rig {
    root: PathBuf,
    cache: PathBuf,
    pipeline: IndexPipeline,
    index: Arc<ItemIndex>,
    store: Arc<MetadataStore>,
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
            web_dist: None,
        };
        let pipeline = IndexPipeline::new(
            paths.clone(),
            config,
            store.clone(),
            index.clone(),
            thumbs,
            bus,
            scanner,
            migrator,
            prefs,
            worker.clone(),
            startup.clone(),
            settings,
        );
        worker.attach(index.clone(), store.clone(), pipeline.sender());
        pipeline.start();
        startup.mark_ready();

        Rig { root, cache, pipeline, index, store }
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

    // 同内容复制到另一路径：内容寻址收敛到同一 item，仅多登记一个位置
    rig.copy_file("a.png", "b.png");
    let third = rig.pipeline.submit_upsert(rig.abs("b.png"), None).await.unwrap();
    let third = third.expect("同内容新路径应登记位置");
    assert_eq!(third.item.id, id, "同内容应收敛为同一 item");
    assert_eq!(rig.index.count(), 1);
    assert_eq!(rig.index.library_location_count(&id), 2);
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

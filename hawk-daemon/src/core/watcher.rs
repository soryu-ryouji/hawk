//! 文件系统监听（notify 封装）。事件统一回调给索引流水线；
//! .hawk/ 内部（回收站除外）与 config.toml 之外的 hawk 自身文件不产生索引事件。
//! notify 原生事件是粒度化的（Create/Remove/Modify/Name(From|To|Both)），
//! 此处折叠为 FileSystemWatcher 语义的 upsert/delete/move；From/To 配对带 300ms 超时兜底。

use crate::core::paths::LibraryPaths;
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum WatcherEvent {
    /// 文件创建/内容变更（绝对路径）
    FileUpsert(String),
    /// 目录创建（绝对路径）——空目录不产生 item 事件,经 folder.changed 通知客户端刷新文件夹树
    FolderCreated(String),
    /// 文件或目录删除（绝对路径；流水线按路径与目录前缀双重匹配处理）
    Deleted(String),
    /// 移动/重命名(旧绝对路径,新绝对路径;目录移动走 DirMoveJob,同时广播 folder.changed)
    Moved { old: String, new: String },
    /// config.toml 变更
    ConfigChanged,
    /// categories.toml / tags.toml 注册表变更（含外部同步写入）
    RegistryChanged,
    /// view.toml 视图偏好变更（含外部同步写入）
    PreferencesChanged,
    /// global_filter.toml 隐藏项注册表变更（含外部同步写入）
    GlobalFilterChanged,
    /// 事件缓冲溢出，需要全量扫描兜底
    Overflow,
}

type Callback = Arc<dyn Fn(WatcherEvent) + Send + Sync>;

const RENAME_PAIR_TIMEOUT: Duration = Duration::from_millis(300);

pub struct LibraryWatcher {
    paths: LibraryPaths,
    callback: Callback,
    _watcher: Mutex<Option<RecommendedWatcher>>,
    pending_from: Arc<Mutex<HashMap<String, Instant>>>,
}

impl LibraryWatcher {
    pub fn new(paths: LibraryPaths, callback: Callback) -> Arc<LibraryWatcher> {
        Arc::new(LibraryWatcher {
            paths,
            callback,
            _watcher: Mutex::new(None),
            pending_from: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn start(self: &Arc<Self>) {
        let paths = self.paths.clone();
        let cb = self.callback.clone();
        let pending_from = self.pending_from.clone();
        let dispatch_paths = paths.clone();
        let dispatch_cb = cb.clone();

        let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => dispatch_event(&dispatch_paths, &dispatch_cb, &pending_from, event),
                Err(e) => {
                    tracing::warn!("文件监听缓冲溢出，触发全量扫描兜底: {e}");
                    dispatch_cb(WatcherEvent::Overflow);
                }
            }
        })
        .expect("创建文件监听失败");

        watcher
            .watch(std::path::Path::new(&paths.root), RecursiveMode::Recursive)
            .expect("监听素材库目录失败");
        *self._watcher.lock().unwrap() = Some(watcher);

        // From/To 配对超时兜底：flush 滞留的 From 为删除（配对任务独立于事件流，无事件也能收敛）
        let this = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_millis(150));
            loop {
                ticker.tick().await;
                this.flush_pending_from();
            }
        });
    }

    /// 滞留超过 RENAME_PAIR_TIMEOUT 的 From 路径按删除处理
    fn flush_pending_from(&self) {
        let stale: Vec<String> = {
            let mut pending = self.pending_from.lock().unwrap();
            let now = Instant::now();
            let stale: Vec<String> = pending
                .iter()
                .filter(|(_, at)| now.duration_since(**at) >= RENAME_PAIR_TIMEOUT)
                .map(|(p, _)| p.clone())
                .collect();
            for p in &stale {
                pending.remove(p);
            }
            stale
        };
        for path in stale {
            if !self.is_internal(&path) {
                (self.callback)(WatcherEvent::Deleted(path));
            }
        }
    }

    fn is_internal(&self, abs: &str) -> bool {
        match self.paths.to_relative(abs) {
            None => true,
            Some(rel) => LibraryPaths::is_internal(&rel),
        }
    }
}

fn dispatch_event(
    paths: &LibraryPaths,
    cb: &Callback,
    pending_from: &Arc<Mutex<HashMap<String, Instant>>>,
    event: Event,
) {
    // 配对超时兜底：每次有事件时顺带 flush（与周期 flush 互补，降低延迟）
    flush_stale(paths, cb, pending_from);

    match event.kind {
        EventKind::Create(_) => {
            for path in event.paths {
                let abs = normalize(&path);
                dispatch_upsert(paths, cb, &abs);
            }
        }
        EventKind::Modify(ModifyKind::Data(_))
        | EventKind::Modify(ModifyKind::Metadata(_))
        | EventKind::Modify(ModifyKind::Any) => {
            // 目录不处理 Changed（内容无意义），仅文件
            for path in event.paths {
                let abs = normalize(&path);
                if std::path::Path::new(&abs).is_file() {
                    dispatch_upsert(paths, cb, &abs);
                }
            }
        }
        EventKind::Modify(ModifyKind::Name(mode)) => match mode {
            RenameMode::Both | RenameMode::Any => {
                if event.paths.len() >= 2 {
                    let old = normalize(&event.paths[0]);
                    let new = normalize(&event.paths[1]);
                    if !is_internal_path(paths, &old) && !is_internal_path(paths, &new) {
                        cb(WatcherEvent::Moved { old, new });
                    }
                }
            }
            RenameMode::From => {
                if let Some(path) = event.paths.first() {
                    pending_from
                        .lock()
                        .unwrap()
                        .insert(normalize(path), Instant::now());
                }
            }
            RenameMode::To => {
                if let Some(path) = event.paths.first() {
                    let new = normalize(path);
                    // 与滞留的 From 配对（rename 对在时间上相邻；并发多 rename 错配由
                    // 幂等流水线 + 超时 flush 兜底自愈）
                    let old = {
                        let mut pending = pending_from.lock().unwrap();
                        let old = pending.keys().next().cloned();
                        if let Some(old) = &old {
                            pending.remove(old);
                        }
                        old
                    };
                    match old {
                        Some(old) if !is_internal_path(paths, &old) && !is_internal_path(paths, &new) => {
                            cb(WatcherEvent::Moved { old, new });
                        }
                        Some(_) => {
                            if !is_internal_path(paths, &new) {
                                dispatch_upsert(paths, cb, &new);
                            }
                        }
                        None => {
                            if !is_internal_path(paths, &new) {
                                dispatch_upsert(paths, cb, &new);
                            }
                        }
                    }
                }
            }
            _ => {}
        },
        EventKind::Remove(_) => {
            for path in event.paths {
                let abs = normalize(&path);
                if !is_internal_path(paths, &abs) {
                    cb(WatcherEvent::Deleted(abs));
                }
            }
        }
        _ => {}
    }
}

/// From/To 的 key 约定：From 存旧路径，To 到来时按「任意待配对 From」消费
fn flush_stale(paths: &LibraryPaths, cb: &Callback, pending_from: &Arc<Mutex<HashMap<String, Instant>>>) {
    let stale: Vec<String> = {
        let mut pending = pending_from.lock().unwrap();
        let now = Instant::now();
        let stale: Vec<String> = pending
            .iter()
            .filter(|(_, at)| now.duration_since(**at) >= RENAME_PAIR_TIMEOUT)
            .map(|(p, _)| p.clone())
            .collect();
        for p in &stale {
            pending.remove(p);
        }
        stale
    };
    for path in stale {
        if !is_internal_path(paths, &path) {
            cb(WatcherEvent::Deleted(path));
        }
    }
}

fn dispatch_upsert(paths: &LibraryPaths, cb: &Callback, abs: &str) {
    let norm_config = normalize_str(&paths.config_file);
    let norm_categories = normalize_str(&paths.categories_file);
    let norm_tags = normalize_str(&paths.tags_file);
    let norm_view = normalize_str(&paths.view_file);
    let norm_global_filter = normalize_str(&paths.global_filter_file);
    if abs == norm_config {
        cb(WatcherEvent::ConfigChanged);
        return;
    }
    if abs == norm_categories || abs == norm_tags {
        cb(WatcherEvent::RegistryChanged);
        return;
    }
    if abs == norm_view {
        cb(WatcherEvent::PreferencesChanged);
        return;
    }
    if abs == norm_global_filter {
        cb(WatcherEvent::GlobalFilterChanged);
        return;
    }
    if is_internal_path(paths, abs) {
        return;
    }
    // 目录不产生 item 事件,单独上报以驱动 folder.changed(目录删除的信号处理：含内容/有设置的目录
    // 由 do_delete 判定广播，空目录由 bootstrap Deleted 分支以目录树缓存判定)
    if std::path::Path::new(abs).is_dir() {
        cb(WatcherEvent::FolderCreated(abs.to_string()));
        return;
    }
    cb(WatcherEvent::FileUpsert(abs.to_string()));
}

fn is_internal_path(paths: &LibraryPaths, abs: &str) -> bool {
    match paths.to_relative(abs) {
        None => true,
        Some(rel) => LibraryPaths::is_internal(&rel),
    }
}

fn normalize(path: &PathBuf) -> String {
    normalize_str(&path.to_string_lossy())
}

fn normalize_str(s: &str) -> String {
    s.replace('\\', "/")
}


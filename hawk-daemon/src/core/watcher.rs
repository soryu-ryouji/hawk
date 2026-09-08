//! 文件系统监听（notify 封装）。事件统一回调给索引流水线；
//! .hawk/ 内部（回收站除外）与 config.toml 之外的 hawk 自身文件不产生索引事件。
//! notify 原生事件是粒度化的（Create/Remove/Modify/Name(From|To|Both)），
//! 此处折叠为 FileSystemWatcher 语义的 upsert/delete/move；From/To 配对带 300ms 超时兜底。

use crate::core::config::LibraryConfig;
use crate::core::paths::LibraryPaths;
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::Path;
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
    config: Arc<LibraryConfig>,
    callback: Callback,
    _watcher: Mutex<Option<RecommendedWatcher>>,
    pending_from: Arc<Mutex<HashMap<String, Instant>>>,
}

impl LibraryWatcher {
    pub fn new(
        paths: LibraryPaths,
        config: Arc<LibraryConfig>,
        callback: Callback,
    ) -> Arc<LibraryWatcher> {
        Arc::new(LibraryWatcher {
            paths,
            config,
            callback,
            _watcher: Mutex::new(None),
            pending_from: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn start(self: &Arc<Self>) {
        let paths = self.paths.clone();
        let config = self.config.clone();
        let cb = self.callback.clone();
        let pending_from = self.pending_from.clone();
        let dispatch_paths = paths.clone();
        let dispatch_config = config.clone();
        let dispatch_cb = cb.clone();

        let mut watcher: RecommendedWatcher =
            notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
                Ok(event) => dispatch_event(
                    &dispatch_paths,
                    &dispatch_config,
                    &dispatch_cb,
                    &pending_from,
                    event,
                ),
                Err(e) => {
                    tracing::warn!("文件监听缓冲溢出，触发全量扫描兜底: {e}");
                    dispatch_cb(WatcherEvent::Overflow);
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
            if !self.is_excluded(&path) {
                (self.callback)(WatcherEvent::Deleted(path));
            }
        }
    }

    /// 索引无关路径：.hawk 内部、含隐藏组件（.DS_Store/.stfolder 等）或扩展名不在可见白名单内
    fn is_excluded(&self, abs: &str) -> bool {
        match self.paths.to_relative(abs) {
            None => true,
            Some(rel) => {
                LibraryPaths::is_internal(&rel)
                    || LibraryPaths::is_hidden(&rel)
                    || !self.config.is_extension_included(&rel)
            }
        }
    }
}

fn dispatch_event(
    paths: &LibraryPaths,
    config: &Arc<LibraryConfig>,
    cb: &Callback,
    pending_from: &Arc<Mutex<HashMap<String, Instant>>>,
    event: Event,
) {
    // 系统明确告知事件被丢弃（macOS FSEvents must-scan-subdirs / 内核丢弃 → Flag::Rescan）：
    // 这是「漏事件」的最强信号，直接走溢出兜底（消费循环排队去重的强制遍历）
    if event.need_rescan() {
        tracing::warn!("文件系统事件被丢弃（need-rescan 标志），触发兜底扫描");
        cb(WatcherEvent::Overflow);
        return;
    }

    // 配对超时兜底：每次有事件时顺带 flush（与周期 flush 互补，降低延迟）
    flush_stale(paths, config, cb, pending_from);

    match event.kind {
        EventKind::Create(_) => {
            for path in event.paths {
                let abs = normalize(&path);
                dispatch_upsert(paths, config, cb, &abs);
            }
        }
        EventKind::Modify(ModifyKind::Data(_))
        | EventKind::Modify(ModifyKind::Metadata(_))
        | EventKind::Modify(ModifyKind::Any) => {
            // 目录不处理 Changed（内容无意义），仅文件
            for path in event.paths {
                let abs = normalize(&path);
                if std::path::Path::new(&abs).is_file() {
                    dispatch_upsert(paths, config, cb, &abs);
                }
            }
        }
        EventKind::Modify(ModifyKind::Name(mode)) => match mode {
            RenameMode::Both | RenameMode::Any => {
                if event.paths.len() >= 2 {
                    let old = normalize(&event.paths[0]);
                    let new = normalize(&event.paths[1]);
                    if !is_excluded_path(paths, config, &old)
                        && !is_excluded_path(paths, config, &new)
                    {
                        cb(WatcherEvent::Moved { old, new });
                    } else {
                        // 任一端隐藏/内部：两端都可见才配对成 Moved，否则按 upsert/删除收敛
                        dispatch_rename_result(paths, config, cb, &old, &new);
                    }
                } else if let Some(path) = event.paths.first() {
                    // macOS FSEvents 的 rename 无法配对：旧/新路径各发一条单路径 Name(Any)。
                    // 以磁盘现状定端：路径已不在 = 旧端（进配对等待，超时按删除）；存在 = 新端
                    // （配对为移动，否则入库）——临时文件 + rename 落盘（Finder/ditto/原子保存）
                    // 的新名字由此入库
                    let abs = normalize(path);
                    if std::path::Path::new(&abs).exists() {
                        pair_or_upsert(paths, config, cb, pending_from, abs);
                    } else {
                        pending_from.lock().unwrap().insert(abs, Instant::now());
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
                    pair_or_upsert(paths, config, cb, pending_from, normalize(path));
                }
            }
            _ => {}
        },
        EventKind::Remove(_) => {
            for path in event.paths {
                let abs = normalize(&path);
                if !is_excluded_path(paths, config, &abs) {
                    cb(WatcherEvent::Deleted(abs));
                }
            }
        }
        _ => {}
    }
}

/// 新路径出现（rename 的新端）：与滞留的旧端配对（rename 对在时间上相邻；并发多 rename
/// 错配由幂等流水线 + 超时 flush 兜底自愈）；无旧端则按新建入库。
/// 来源：其他平台的 RenameMode::To 与 macOS FSEvents 单路径 Name(Any) 的新端
fn pair_or_upsert(
    paths: &LibraryPaths,
    config: &Arc<LibraryConfig>,
    cb: &Callback,
    pending_from: &Arc<Mutex<HashMap<String, Instant>>>,
    new: String,
) {
    let old = {
        let mut pending = pending_from.lock().unwrap();
        let old = pending.keys().next().cloned();
        if let Some(old) = &old {
            pending.remove(old);
        }
        old
    };
    match old {
        Some(old)
            if !is_excluded_path(paths, config, &old) && !is_excluded_path(paths, config, &new) =>
        {
            cb(WatcherEvent::Moved { old, new });
        }
        Some(old) => dispatch_rename_result(paths, config, cb, &old, &new),
        None => {
            if !is_excluded_path(paths, config, &new) {
                dispatch_upsert(paths, config, cb, &new);
            }
        }
    }
}

/// From/To 的 key 约定：From 存旧路径，To 到来时按「任意待配对 From」消费
fn flush_stale(
    paths: &LibraryPaths,
    config: &Arc<LibraryConfig>,
    cb: &Callback,
    pending_from: &Arc<Mutex<HashMap<String, Instant>>>,
) {
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
        if !is_excluded_path(paths, config, &path) {
            cb(WatcherEvent::Deleted(path));
        }
    }
}

fn dispatch_upsert(paths: &LibraryPaths, config: &Arc<LibraryConfig>, cb: &Callback, abs: &str) {
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
    if is_excluded_path(paths, config, abs) {
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

/// 索引无关路径：.hawk 内部或含隐藏组件（.DS_Store、.stfolder 等）。
/// 这些路径不产生 upsert/移动事件；删除事件同样不发出——
/// 隐藏项本就不该在索引中，旧残留由扫描的 Remove/消失对账收敛
fn is_excluded_path(paths: &LibraryPaths, config: &Arc<LibraryConfig>, abs: &str) -> bool {
    match paths.to_relative(abs) {
        None => true,
        Some(rel) => {
            LibraryPaths::is_internal(&rel)
                || LibraryPaths::is_hidden(&rel)
                || !config.is_extension_included(&rel)
        }
    }
}

/// rename 收尾：new 可见则 upsert，否则（new 隐藏/内部）在 old 可见时按删除处理
/// ——从索引位置移入隐藏目录不能残留 old 位置的索引
fn dispatch_rename_result(
    paths: &LibraryPaths,
    config: &Arc<LibraryConfig>,
    cb: &Callback,
    old: &str,
    new: &str,
) {
    if !is_excluded_path(paths, config, new) {
        dispatch_upsert(paths, config, cb, new);
    } else if !is_excluded_path(paths, config, old) {
        cb(WatcherEvent::Deleted(old.to_string()));
    }
}

fn normalize(path: &Path) -> String {
    normalize_str(&path.to_string_lossy())
}

fn normalize_str(s: &str) -> String {
    s.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// 记录回调：收集派发的事件
    fn recorder() -> (Callback, Arc<Mutex<Vec<WatcherEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let cb: Callback = Arc::new(move |e| sink.lock().unwrap().push(e));
        (cb, events)
    }

    /// 临时库装配（watcher 只用到路径规则、配置与磁盘现状，不需要完整流水线）
    fn rig(name: &str) -> (LibraryPaths, Arc<LibraryConfig>, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("hawk-watcher-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let paths = LibraryPaths::new(base.to_str().unwrap(), None);
        paths.ensure_layout();
        let config = Arc::new(LibraryConfig::new(paths.clone()));
        (paths, config, base)
    }

    /// macOS FSEvents 的 rename 事件：单路径 Modify(Name(Any))
    fn name_event(path: &str) -> Event {
        Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Any)))
            .add_path(PathBuf::from(path))
    }

    fn rel(path: &str) -> String {
        path.replace('\\', "/")
    }

    /// 临时文件 + rename 落盘（Finder/ditto/原子保存）：新名字路径的事件必须入库
    #[test]
    fn single_path_rename_upserts_materialized_file() {
        let (paths, config, root) = rig("rename-upsert");
        let file = root.join("a.png");
        std::fs::write(&file, b"x").unwrap();
        let (cb, events) = recorder();
        let pending = Arc::new(Mutex::new(HashMap::new()));

        dispatch_event(
            &paths,
            &config,
            &cb,
            &pending,
            name_event(file.to_str().unwrap()),
        );

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], WatcherEvent::FileUpsert(p) if p.ends_with("a.png")));
    }

    /// 旧路径（已不存在）进配对，新路径（存在）配对成 Moved——元数据随位置跟随，不重算哈希
    #[test]
    fn single_path_rename_pairs_into_move() {
        let (paths, config, root) = rig("rename-pair");
        let old = root.join("b.png");
        let new = root.join("b2.png");
        std::fs::write(&new, b"x").unwrap();
        let (cb, events) = recorder();
        let pending = Arc::new(Mutex::new(HashMap::new()));

        dispatch_event(
            &paths,
            &config,
            &cb,
            &pending,
            name_event(old.to_str().unwrap()),
        );
        assert!(events.lock().unwrap().is_empty(), "旧端不应立即产生事件");

        dispatch_event(
            &paths,
            &config,
            &cb,
            &pending,
            name_event(new.to_str().unwrap()),
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], WatcherEvent::Moved { old: o, new: n } if o.ends_with("b.png") && n.ends_with("b2.png")),
            "{events:?}"
        );
    }

    /// 移出库/删除：旧端无配对，超时后按删除收敛
    #[test]
    fn single_path_rename_flushes_stale_as_deleted() {
        let (paths, config, root) = rig("rename-delete");
        let gone = root.join("c.png");
        let (cb, events) = recorder();
        let pending = Arc::new(Mutex::new(HashMap::new()));

        dispatch_event(
            &paths,
            &config,
            &cb,
            &pending,
            name_event(gone.to_str().unwrap()),
        );
        pending.lock().unwrap().insert(
            rel(gone.to_str().unwrap()),
            Instant::now() - RENAME_PAIR_TIMEOUT,
        );
        flush_stale(&paths, &config, &cb, &pending);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], WatcherEvent::Deleted(p) if p.ends_with("c.png")));
    }

    /// 事件被系统丢弃（Flag::Rescan）→ 溢出兜底（强制遍历收敛）
    #[test]
    fn rescan_flag_triggers_overflow() {
        let (paths, config, _root) = rig("rescan-flag");
        let (cb, events) = recorder();
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let event = Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        dispatch_event(&paths, &config, &cb, &pending, event);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], WatcherEvent::Overflow));
    }

    /// 隐藏端不产生事件：临时文件名（.BC.T_xxx 等）与可见新名字配对时按 upsert 收敛
    #[test]
    fn single_path_rename_from_hidden_temp_upserts_new_name() {
        let (paths, config, root) = rig("rename-hidden");
        let temp = root.join(".BC.T_abc");
        let file = root.join("d.png");
        std::fs::write(&file, b"x").unwrap();
        let (cb, events) = recorder();
        let pending = Arc::new(Mutex::new(HashMap::new()));

        dispatch_event(
            &paths,
            &config,
            &cb,
            &pending,
            name_event(temp.to_str().unwrap()),
        );
        dispatch_event(
            &paths,
            &config,
            &cb,
            &pending,
            name_event(file.to_str().unwrap()),
        );

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], WatcherEvent::FileUpsert(p) if p.ends_with("d.png")),
            "{events:?}"
        );
    }
}

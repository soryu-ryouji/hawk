//! 素材目录遍历。只读目录项信息，不读文件内容。
//! 跳过 .hawk/ 内部（回收站 .hawk/trash/ 除外），库内子树应用 config.toml 的 ignore 规则。

use crate::core::config::LibraryConfig;
use crate::core::paths::{unix_ms, LibraryPaths};

pub struct LibraryScanner {
    paths: LibraryPaths,
    config: std::sync::Arc<LibraryConfig>,
}

impl LibraryScanner {
    pub fn new(paths: LibraryPaths, config: std::sync::Arc<LibraryConfig>) -> LibraryScanner {
        LibraryScanner { paths, config }
    }

    /// 遍历指定目录（目录创建事件后的补扫），产出全部文件绝对路径
    pub fn walk_directory(&self, abs_dir: &str) -> Vec<String> {
        let is_trash_subtree = self.is_trash_path(abs_dir);
        let mut out = Vec::new();
        let mut pending = vec![abs_dir.to_string()];
        while let Some(dir) = pending.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let full = entry.path().to_string_lossy().replace('\\', "/");
                let rel = match self.paths.to_relative(&full) {
                    Some(r) => r,
                    None => continue,
                };
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    if rel == LibraryPaths::HAWK_DIR_NAME {
                        pending.push(self.paths.trash_dir.clone());
                        continue;
                    }
                    let in_trash = is_trash_subtree || LibraryPaths::is_in_trash(&format!("{rel}/"));
                    if !in_trash && self.config.is_ignored(&rel) {
                        continue;
                    }
                    pending.push(full);
                } else if !is_trash_subtree && self.config.is_ignored(&rel) {
                    continue;
                } else {
                    out.push(full);
                }
            }
        }
        out
    }

    /// 产出全库目录及 (mtime, 直接子项数)——增量扫描的快照对比输入。
    /// entries 为原始直接子项数（不过 ignore）：快照对比追求「任何变化都触发深入」
    pub fn walk_directory_stats(&self, walk_incomplete: &std::sync::atomic::AtomicBool) -> Vec<(String, i64, i64)> {
        let mut out = Vec::new();
        let mut pending = vec![self.paths.root.clone()];
        while let Some(dir) = pending.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => {
                    // 目录枚举失败（权限不足/遍历期间被删等瞬时错误）：调用方据此判定遍历不完整
                    walk_incomplete.store(true, std::sync::atomic::Ordering::SeqCst);
                    continue;
                }
            };
            let mut count: i64 = 0;
            for entry in entries.flatten() {
                count += 1;
                let full = entry.path().to_string_lossy().replace('\\', "/");
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if !is_dir {
                    continue;
                }
                if let Some(rel) = self.paths.to_relative(&full) {
                    if rel == LibraryPaths::HAWK_DIR_NAME {
                        pending.push(self.paths.trash_dir.clone());
                        continue;
                    }
                    pending.push(full);
                }
            }
            if let Some(rel) = self.paths.to_relative(&dir) {
                // GetLastWriteTimeUtc 对遍历期间被删的目录返回旧时间而非抛异常：快照必然不一致 → 深入时目录已消失，深入枚举容忍即可
                let mtime = std::fs::metadata(&dir)
                    .and_then(|m| m.modified())
                    .map(unix_ms)
                    .unwrap_or(0);
                out.push((rel, mtime, count));
            }
        }
        out
    }

    /// 只枚举目录的直接文件（不深入子目录）——增量扫描按目录深入时用
    pub fn walk_files_in_directory(&self, abs_dir: &str) -> Vec<String> {
        let entries = match std::fs::read_dir(abs_dir) {
            Ok(e) => e,
            Err(_) => return Vec::new(), // 权限不足或遍历期间被删除
        };
        let is_trash_subtree = self.is_trash_path(abs_dir);
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let full = entry.path().to_string_lossy().replace('\\', "/");
            let rel = match self.paths.to_relative(&full) {
                Some(r) => r,
                None => continue,
            };
            if !is_trash_subtree && self.config.is_ignored(&rel) {
                continue;
            }
            out.push(full);
        }
        out
    }

    fn is_trash_path(&self, abs_dir: &str) -> bool {
        self.paths
            .to_relative(abs_dir)
            .map(|rel| LibraryPaths::is_in_trash(&format!("{rel}/")))
            .unwrap_or(false)
    }
}

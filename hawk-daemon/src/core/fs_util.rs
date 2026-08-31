//! 库内文件系统操作辅助。

use crate::core::paths::LibraryPaths;

/// 确保文件的父目录存在
pub fn ensure_parent_dir(abs_file: &str) {
    if let Some(parent) = std::path::Path::new(abs_file).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
}

/// 计算回收站中的可用位置（保留原目录结构）。
/// 同名冲突时在文件名后追加 " (n)" 后缀——恢复时按回收站中的实际名称放回。
pub fn find_free_trash_path(paths: &LibraryPaths, library_rel_path: &str, is_directory: bool) -> String {
    let mut candidate = LibraryPaths::library_to_trash_path(library_rel_path);
    for n in 1.. {
        let abs = paths.to_absolute(&candidate);
        let free = abs
            .as_ref()
            .map(|a| !std::path::Path::new(a).exists())
            .unwrap_or(false);
        if free {
            return abs.unwrap();
        }
        candidate = LibraryPaths::library_to_trash_path(&suffixed(library_rel_path, n, is_directory));
    }
    unreachable!()
}

fn suffixed(rel_path: &str, n: u32, is_directory: bool) -> String {
    let dir = LibraryPaths::dir_of(rel_path);
    let name = LibraryPaths::name_of(rel_path);
    let ext = LibraryPaths::ext_of(rel_path);
    let new_name = if is_directory || ext.is_empty() {
        format!("{name} ({n})")
    } else {
        format!("{name} ({n}).{ext}")
    };
    if dir.is_empty() {
        new_name
    } else {
        format!("{dir}/{new_name}")
    }
}

/// 校验文件夹/文件名（不允许路径分隔符与特殊名称）
pub fn is_valid_name(name: Option<&str>) -> bool {
    match name {
        Some(n) => {
            !n.trim().is_empty()
                && !n.contains('/')
                && !n.contains('\\')
                && n != "."
                && n != ".."
                && n != LibraryPaths::HAWK_DIR_NAME
        }
        None => false,
    }
}

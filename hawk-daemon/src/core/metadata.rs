//! 素材元数据模型：权威持久层为 `.hawk/metadata.db`（见 metadata_db.rs）。
//! 这里只定义内存与 SQLite 之间的数据结构，不再提供 TOML 解析/序列化路径。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq)]
pub struct PathEntry {
    pub path: String,
    pub size: i64,
    pub modification_time: i64,
}

/// 调色板条目（SQLite JSON 镜像格式）
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PaletteEntry {
    pub color: String,
    pub percentage: f32,
}

/// 内容感知哈希。version=算法版本；value 为 64 位值，None 表示已探测的非图像负缓存。
#[derive(Clone, Debug, PartialEq)]
pub struct PerceptualHash {
    pub version: u32,
    pub value: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemMetadata {
    pub paths: Vec<PathEntry>,
    pub url: Option<String>,
    pub tags: Vec<String>,
    pub categories: Vec<String>,
    pub star: i32,
    pub annotation: Option<String>,
    /// 图像宽（像素）；0 = 未知/非图像
    pub width: i32,
    /// 图像高（像素）；0 = 未知/非图像
    pub height: i32,
    /// 调色板（按占比降序，最多 10 色）；None = 未提炼，Some(vec![]) = 非图像负缓存
    pub palette: Option<Vec<PaletteEntry>>,
    /// pHash；None = 尚未计算，Some(value=None) = 已探测的非图像负缓存
    pub phash: Option<PerceptualHash>,
}

impl ItemMetadata {
    /// 派生探测负缓存：空调色板 = 已探测且非可解码图像
    pub fn palette_negative_cached(&self) -> bool {
        self.palette.as_ref().is_some_and(|p| p.is_empty())
    }

    pub fn find_path(&self, path: &str) -> Option<&PathEntry> {
        self.paths.iter().find(|p| p.path == path)
    }

    pub fn find_path_mut(&mut self, path: &str) -> Option<&mut PathEntry> {
        self.paths.iter_mut().find(|p| p.path == path)
    }
}

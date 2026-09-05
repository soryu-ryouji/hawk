//! 内存索引中的 item：位置列表 + 元数据副本 + 宽高派生信息。
//! tags/star/annotation/url 以元数据为准，此处为查询用副本，由流水线单向同步。
//! 含回收站视图投影规则。

use crate::core::color_math::{rgb_to_lab, LabColor};
use crate::core::metadata::ItemMetadata;
use crate::core::paths::LibraryPaths;
use serde::Serialize;

/// 调色板中的一个颜色：RGB 用于展示与缓存，Lab 为预算的检索坐标
#[derive(Clone, Debug)]
pub struct PaletteColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub percentage: f32,
    pub lab: LabColor,
}

impl PaletteColor {
    pub fn from_rgb(r: u8, g: u8, b: u8, percentage: f32) -> PaletteColor {
        PaletteColor {
            r,
            g,
            b,
            percentage,
            lab: rgb_to_lab(r, g, b),
        }
    }
}

/// item 的一个文件位置。Path 为相对库根目录路径；回收站位置以 .hawk/trash/ 开头。
#[derive(Clone, Debug)]
pub struct ItemLocation {
    pub path: String,
    pub size: i64,
    pub modification_time: i64,
}

impl ItemLocation {
    pub fn in_trash(&self) -> bool {
        LibraryPaths::is_in_trash(&self.path)
    }

    /// 对应的库内路径（回收站位置去掉前缀，即删除前的原路径）
    pub fn library_path(&self) -> &str {
        LibraryPaths::trash_to_library_path(&self.path)
    }
}

#[derive(Default)]
pub struct Item {
    pub id: String,
    pub locations: Vec<ItemLocation>,
    pub url: Option<String>,
    pub tags: Vec<String>,
    pub categories: Vec<String>,
    pub star: i32,
    pub annotation: Option<String>,
    pub width: i32,
    pub height: i32,
    /// 提炼的调色板（按占比降序，最多 10 个）；尚未提炼或不支持解码时为空
    pub palette: Vec<PaletteColor>,
}

impl Item {
    pub fn has_library_locations(&self) -> bool {
        self.locations.iter().any(|l| !l.in_trash())
    }

    /// 用元数据刷新查询副本（元数据 → 索引的单向同步，只允许索引流水线调用）
    pub fn sync_from(&mut self, meta: &ItemMetadata) {
        self.url = meta.url.clone();
        self.tags = meta.tags.clone();
        self.categories = meta.categories.clone();
        self.star = meta.star;
        self.annotation = meta.annotation.clone();
        self.width = meta.width;
        self.height = meta.height;
        // Palette 是内容的纯函数，直接随元数据同步；未提炼（None）保持索引现状
        if let Some(palette) = &meta.palette {
            self.palette = palette
                .iter()
                .filter_map(|p| {
                    crate::core::color_math::parse_hex(Some(&p.color))
                        .map(|(r, g, b)| PaletteColor::from_rgb(r, g, b, p.percentage))
                })
                .collect();
        }
    }

    /// 主位置：普通视图取首个库内位置，回收站视图取首个回收站位置
    pub fn main_location(&self, trash_view: bool) -> Option<&ItemLocation> {
        if trash_view {
            self.locations.iter().find(|l| l.in_trash())
        } else {
            self.locations.iter().find(|l| !l.in_trash())
        }
    }

    /// 投影为 API 的 Item 对象（主位置口径：事件载荷/单条 detail 缺省用）。回收站视图的 paths 展示原库内路径（恢复目标）。
    pub fn to_dto(&self, trash_view: bool) -> ItemDto {
        let main = self.main_location(trash_view).expect("to_dto: item must have a location in the view");
        self.to_dto_at(main, trash_view)
    }

    /// 投影指定位置的 DTO：同内容多位置各自成卡（name/ext/size/mtime 取该位置），
    /// paths/folders 仍投影该视图侧的全部位置（检查器展示同内容的完整分布）。
    pub fn to_dto_at(&self, loc: &ItemLocation, trash_view: bool) -> ItemDto {
        let locations: Vec<&ItemLocation> = self.locations.iter().filter(|l| l.in_trash() == trash_view).collect();
        let paths: Vec<String> = locations
            .iter()
            .map(|l| {
                if trash_view {
                    l.library_path().to_string()
                } else {
                    l.path.clone()
                }
            })
            .collect();
        let mut folders: Vec<String> = Vec::new();
        for p in &paths {
            let dir = LibraryPaths::dir_of(p);
            if !dir.is_empty() && !folders.iter().any(|f| f == dir) {
                folders.push(dir.to_string());
            }
        }

        ItemDto {
            id: self.id.clone(),
            path: loc.path.clone(),
            name: LibraryPaths::name_of(loc.library_path()).to_string(),
            ext: LibraryPaths::ext_of(loc.library_path()),
            width: self.width,
            height: self.height,
            size: loc.size,
            url: self.url.clone(),
            tags: self.tags.clone(),
            categories: self.categories.clone(),
            paths,
            folders,
            star: self.star,
            annotation: self.annotation.clone(),
            modification_time: loc.modification_time,
            palette: self
                .palette
                .iter()
                .map(|p| PaletteColorDto {
                    color: crate::core::color_math::to_hex(p.r, p.g, p.b),
                    percentage: p.percentage,
                })
                .collect(),
        }
    }
}

/// API 的调色板颜色项
#[derive(Serialize, utoipa::ToSchema)]
pub struct PaletteColorDto {
    /// # 前缀小写 hex，如 "#344441"
    pub color: String,
    /// 像素覆盖占比（0–100，1 位小数）
    pub percentage: f32,
}

/// API 的 Item 对象（snake_case）
#[derive(Serialize, utoipa::ToSchema)]
pub struct ItemDto {
    pub id: String,
    /// 本条目对应的库内相对位置（同 id 内容多位置时按 path 区分条目；回收站视图为 .hawk/trash/ 实际路径）
    pub path: String,
    pub name: String,
    pub ext: String,
    pub width: i32,
    pub height: i32,
    pub size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub tags: Vec<String>,
    pub categories: Vec<String>,
    pub paths: Vec<String>,
    pub folders: Vec<String>,
    pub star: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotation: Option<String>,
    pub modification_time: i64,
    pub palette: Vec<PaletteColorDto>,
}

/// 网格骨架：虚拟布局所需的最低限度信息（ItemDto 的同序轻量投影）
#[derive(Serialize, utoipa::ToSchema)]
pub struct ItemSkeletonDto {
    pub id: String,
    /// 同 id（内容）多位置时按 path 区分条目
    pub path: String,
    pub width: i32,
    pub height: i32,
    pub star: i32,
    /// 该位置的字节数（位置级）：前端选择集大小聚合用（详情只覆盖视口窗口，承担不起全量聚合）
    pub size: i64,
}

/// item/list 的查询条件（全部可选，组合逻辑为 AND）
#[derive(Default, Clone)]
pub struct ItemQuery {
    pub ids: Option<Vec<String>>,
    pub keywords: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub star: Option<i32>,
    pub folders: Option<Vec<String>>,
    /// 为 true 时文件夹只精确匹配直接位于该目录下的 item（不含子目录）；空字符串表示库根目录
    pub folders_exact: bool,
    pub categories: Option<Vec<String>>,
    pub categories_match: Option<String>,
    pub exclude_categories: Option<Vec<String>>,
    pub exclude_tags: Option<Vec<String>>,
    /// 排除文件夹（位置级，子树整体剔除；任一命中即剔除）。空字符串条目无意义，过滤时忽略
    pub exclude_folders: Option<Vec<String>>,
    pub without_categories: bool,
    pub without_tags: bool,
    pub ext: Option<String>,
    pub annotation: Option<String>,
    pub url: Option<String>,
    /// 颜色检索（已转 Lab）；命中条件为调色板任一颜色 ΔE ≤ 阈值
    pub color: Option<LabColor>,
    pub in_trash: bool,
    pub order_by: Option<String>,
    pub order: Option<String>,
    pub offset: i32,
    pub limit: i32,
}

impl Default for ItemDto {
    fn default() -> Self {
        ItemDto {
            id: String::new(),
            path: String::new(),
            name: String::new(),
            ext: String::new(),
            width: 0,
            height: 0,
            size: 0,
            url: None,
            tags: Vec::new(),
            categories: Vec::new(),
            paths: Vec::new(),
            folders: Vec::new(),
            star: 0,
            annotation: None,
            modification_time: 0,
            palette: Vec::new(),
        }
    }
}



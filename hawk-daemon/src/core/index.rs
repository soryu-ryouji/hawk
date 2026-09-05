//! 内存索引:hash → item,位置路径 → hash 反查。一把 RwLock 保护。
//! 写入只发生在索引流水线(单写者),读取可来自任意 HTTP 线程（RwLock 读并发）。
//! 读取纪律:HTTP 层一律走 get_dto / query / query_skeleton / find_location / main_source_abs /
//! contains / count 等不可变快照与锁内投影;可变引用仅限流水线(经 with_item_mut)。
//! 查询路径「锁内只做过滤与排序键投影，排序在锁外」：读写互相阻塞的时间不含 O(N log N) 排序。
//! 排序稳定:主键同值按 id 字典序打破平局。

use crate::core::color_math::delta_e_squared;
use crate::core::item::{Item, ItemDto, ItemLocation, ItemQuery, ItemSkeletonDto};
use crate::core::paths::LibraryPaths;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

/// 颜色检索的 ΔE 阈值（CIE76）。约覆盖「同一颜色家族」的宽松度
pub const COLOR_MATCH_THRESHOLD: f64 = 25.0;
const COLOR_MATCH_THRESHOLD_SQUARED: f64 = COLOR_MATCH_THRESHOLD * COLOR_MATCH_THRESHOLD;

/// item 文件位置的不可变快照:API 层定位待操作文件的唯一方式
#[derive(Clone, Debug)]
pub struct LocationSnapshot {
    pub path: String,
    pub library_path: String,
    pub in_trash: bool,
}

/// 刷新缓存范围：folder 前缀匹配（含子目录，库内位置）/ 分类 / 标签 / 整库（含回收站）。
/// 供 library/refresh_cache 按范围派发派生缓存修复任务
pub enum RefreshScope {
    Folder(String),
    Category(String),
    Tag(String),
    Library,
}

#[derive(Default)]
struct IndexInner {
    by_hash: HashMap<String, Item>,
    hash_by_location: HashMap<String, String>,
}

#[derive(Default)]
pub struct ItemIndex {
    inner: RwLock<IndexInner>,
}

/// 读锁快捷方式（索引为进程内单例，锁污染即 panic 传播，与此前 Mutex 行为一致）
macro_rules! read_inner {
    ($self:expr) => {
        $self.inner.read().unwrap()
    };
}
macro_rules! write_inner {
    ($self:expr) => {
        $self.inner.write().unwrap()
    };
}

impl ItemIndex {
    /// 索引中是否存在该 item
    pub fn contains(&self, hash: &str) -> bool {
        read_inner!(self).by_hash.contains_key(hash)
    }

    /// 锁内投影为 DTO(trashView 按「是否只剩回收站位置」自动判定),HTTP 层的读取出口。
    /// 零位置 item 不投影（不应存在；防御性返回 None）
    pub fn get_dto(&self, hash: &str) -> Option<ItemDto> {
        let inner = read_inner!(self);
        inner
            .by_hash
            .get(hash)
            .filter(|i| !i.locations.is_empty())
            .map(|i| i.to_dto(!i.has_library_locations()))
    }

    /// 锁内投影指定位置的 DTO（同内容多位置时按 path 定位；path 缺省等同 get_dto 主位置口径）
    pub fn get_dto_at(&self, hash: &str, path: Option<&str>) -> Option<ItemDto> {
        let inner = read_inner!(self);
        let item = inner.by_hash.get(hash).filter(|i| !i.locations.is_empty())?;
        let trash_view = !item.has_library_locations();
        let loc = match path {
            None => item.main_location(trash_view)?,
            Some(p) => item
                .locations
                .iter()
                .find(|l| l.in_trash() == trash_view && (l.path == p || l.library_path() == p))?,
        };
        Some(item.to_dto_at(loc, trash_view))
    }

    /// 宽高是否尚未解析（0 × 0）。只读访问，缩略图 worker 的补宽高闸门用
    pub fn dim_is_zero(&self, hash: &str) -> bool {
        let inner = read_inner!(self);
        inner.by_hash.get(hash).map(|i| i.width == 0).unwrap_or(false)
    }

    /// 取得或创建 item（不存在时创建并登记）。返回是否新建。仅限流水线(单写者)与测试
    pub fn get_or_add(&self, hash: &str) -> bool {
        let mut inner = write_inner!(self);
        if inner.by_hash.contains_key(hash) {
            return false;
        }
        inner.by_hash.insert(
            hash.to_string(),
            Item {
                id: hash.to_string(),
                ..Item::default()
            },
        );
        true
    }

    /// 取得或创建 item，并在同一锁内携带位置登记——创建即有位置，
    /// 避免零位置 item 短暂对并发查询可见（to_dto 取 locations[0] 会越界）。
    /// 返回 (是否新建 item, 是否新增位置)。仅限流水线(单写者)
    pub fn get_or_add_with_location(
        &self,
        hash: &str,
        location_path: &str,
        size: i64,
        mtime: i64,
    ) -> (bool, bool) {
        let mut inner = write_inner!(self);
        let created = !inner.by_hash.contains_key(hash);
        if created {
            inner.by_hash.insert(
                hash.to_string(),
                Item {
                    id: hash.to_string(),
                    ..Item::default()
                },
            );
        }
        let item = inner.by_hash.get_mut(hash).expect("get_or_add_with_location: item must exist");
        let added_location = if let Some(loc) = item.locations.iter_mut().find(|l| l.path == location_path) {
            loc.size = size;
            loc.modification_time = mtime;
            false
        } else {
            item.locations.push(ItemLocation {
                path: location_path.to_string(),
                size,
                modification_time: mtime,
            });
            inner.hash_by_location.insert(location_path.to_string(), hash.to_string());
            true
        };
        (created, added_location)
    }

    /// 可变 item 访问（单写者纪律由调用方保证）
    pub fn with_item_mut<R>(&self, hash: &str, f: impl FnOnce(&mut Item) -> R) -> Option<R> {
        let mut inner = write_inner!(self);
        inner.by_hash.get_mut(hash).map(f)
    }

    /// 登记/刷新一个位置。返回是否为新增位置
    pub fn add_or_update_location(&self, hash: &str, location_path: &str, size: i64, mtime: i64) -> bool {
        let mut inner = write_inner!(self);
        let item = inner.by_hash.get_mut(hash).expect("add_or_update_location: item must exist");
        if let Some(loc) = item.locations.iter_mut().find(|l| l.path == location_path) {
            loc.size = size;
            loc.modification_time = mtime;
            false
        } else {
            item.locations.push(ItemLocation {
                path: location_path.to_string(),
                size,
                modification_time: mtime,
            });
            inner.hash_by_location.insert(location_path.to_string(), hash.to_string());
            true
        }
    }

    /// 移除一个位置；item 不再有任何位置时从索引移除
    pub fn remove_location(&self, location_path: &str) -> Option<String> {
        let mut inner = write_inner!(self);
        let hash = inner.hash_by_location.remove(location_path)?;
        if let Some(item) = inner.by_hash.get_mut(&hash) {
            item.locations.retain(|l| l.path != location_path);
            if item.locations.is_empty() {
                inner.by_hash.remove(&hash);
            }
        }
        Some(hash)
    }

    /// 位置移动（同内容改名/移动/进出回收站，hash 不变）。返回所属 hash，未索引返回 None
    pub fn move_location(&self, old_path: &str, new_path: &str) -> Option<String> {
        let mut inner = write_inner!(self);
        let hash = inner.hash_by_location.remove(old_path)?;
        inner.hash_by_location.insert(new_path.to_string(), hash.clone());
        let item = inner.by_hash.get_mut(&hash).expect("move_location: item must exist");
        let loc = item.locations.iter_mut().find(|l| l.path == old_path).expect("move_location: location must exist");
        loc.path = new_path.to_string();
        Some(hash)
    }

    /// 位置定位并返回不可变快照:缺省为主位置(want_trash=false 取首个库内位置,true 取首个回收站位置);
    /// 指定 path 时按视图匹配(回收站位置以其原库内路径匹配)
    pub fn find_location(&self, hash: &str, path: Option<&str>, want_trash: Option<bool>) -> Option<LocationSnapshot> {
        let inner = read_inner!(self);
        let item = inner.by_hash.get(hash)?;
        let loc = match path {
            None => match want_trash {
                Some(false) => item.locations.iter().find(|l| !l.in_trash()),
                Some(true) => item.locations.iter().find(|l| l.in_trash()),
                None => item
                    .locations
                    .iter()
                    .find(|l| !l.in_trash())
                    .or_else(|| item.locations.first()),
            },
            Some(p) => item.locations.iter().find(|l| {
                (want_trash.is_none() || l.in_trash() == want_trash.unwrap()) && (l.path == p || l.library_path() == p)
            }),
        }?;
        Some(LocationSnapshot {
            path: loc.path.clone(),
            library_path: loc.library_path().to_string(),
            in_trash: loc.in_trash(),
        })
    }

    /// 主位置(优先库内)的源文件绝对路径快照;item/file、refresh_thumbnail 用
    pub fn main_source_abs(&self, hash: &str, paths: &LibraryPaths) -> Option<String> {
        let inner = read_inner!(self);
        let item = inner.by_hash.get(hash)?;
        let loc = item
            .locations
            .iter()
            .find(|l| !l.in_trash())
            .or_else(|| item.locations.first())?;
        paths.to_absolute(&loc.path)
    }

    pub fn hash_by_location(&self, location_path: &str) -> Option<String> {
        read_inner!(self).hash_by_location.get(location_path).cloned()
    }

    /// 指定 item 的全部位置快照（want_trash 过滤，None 为全部）：
    /// 卡片级删除/恢复（无 path 参数）需要枚举同内容多路径 item 的全部位置
    pub fn item_locations(&self, hash: &str, want_trash: Option<bool>) -> Vec<LocationSnapshot> {
        let inner = read_inner!(self);
        let Some(item) = inner.by_hash.get(hash) else {
            return Vec::new();
        };
        item.locations
            .iter()
            .filter(|l| want_trash.is_none() || l.in_trash() == want_trash.unwrap())
            .map(|l| LocationSnapshot {
                path: l.path.clone(),
                library_path: l.library_path().to_string(),
                in_trash: l.in_trash(),
            })
            .collect()
    }

    /// 库内文件总数（位置级：同内容多位置各计一次；不含回收站）
    pub fn count(&self) -> usize {
        let inner = read_inner!(self);
        inner
            .by_hash
            .values()
            .map(|i| i.locations.iter().filter(|l| !l.in_trash()).count())
            .sum()
    }

    /// 指定 item 是否还有库内位置（回收站视图判定用）
    pub fn has_library_location(&self, hash: &str) -> bool {
        let inner = read_inner!(self);
        inner.by_hash.get(hash).map(|i| i.has_library_locations()).unwrap_or(false)
    }

    /// 指定 item 的库内位置数（事件转换判定用）
    pub fn library_location_count(&self, hash: &str) -> usize {
        let inner = read_inner!(self);
        inner
            .by_hash
            .get(hash)
            .map(|i| i.locations.iter().filter(|l| !l.in_trash()).count())
            .unwrap_or(0)
    }

    /// 全部位置路径快照（扫描时做消失检测用）
    pub fn all_location_paths(&self) -> Vec<String> {
        read_inner!(self).hash_by_location.keys().cloned().collect()
    }

    /// 某目录前缀下的全部位置路径快照
    pub fn locations_under(&self, rel_dir_prefix: &str) -> Vec<String> {
        let inner = read_inner!(self);
        inner
            .hash_by_location
            .keys()
            .filter(|p| p.starts_with(rel_dir_prefix))
            .cloned()
            .collect()
    }

    /// 全部分类路径快照（含回收站 item 的赋值，分类树派生用）
    pub fn all_categories(&self) -> Vec<String> {
        let inner = read_inner!(self);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for item in inner.by_hash.values() {
            for c in &item.categories {
                if seen.insert(c.clone()) {
                    out.push(c.clone());
                }
            }
        }
        out
    }

    /// 全部标签及库内计数快照（位置级计数：同内容多位置各计一次；不含回收站）
    pub fn tags_with_counts(&self) -> Vec<(String, usize)> {
        let inner = read_inner!(self);
        let mut counts: HashMap<String, usize> = HashMap::new();
        for item in inner.by_hash.values() {
            let n = item.locations.iter().filter(|l| !l.in_trash()).count();
            if n == 0 {
                continue;
            }
            for t in &item.tags {
                *counts.entry(t.clone()).or_insert(0) += n;
            }
        }
        let mut names: HashSet<String> = HashSet::new();
        for item in inner.by_hash.values() {
            for t in &item.tags {
                names.insert(t.clone());
            }
        }
        names.into_iter().map(|n| (n.clone(), counts.get(&n).copied().unwrap_or(0))).collect()
    }

    /// 按目录统计库内文件数（位置级：同内容多位置各计一次；不含回收站）。key 为目录相对路径（"" 为库根）。
    /// 计数含全部子孙目录（每位置沿目录链各计一次）
    pub fn folder_counts(&self) -> HashMap<String, usize> {
        let inner = read_inner!(self);
        let mut counts: HashMap<String, usize> = HashMap::new();
        for item in inner.by_hash.values() {
            for loc in item.locations.iter().filter(|l| !l.in_trash()) {
                let mut dir = LibraryPaths::dir_of(&loc.path);
                loop {
                    *counts.entry(dir.to_string()).or_insert(0) += 1;
                    if dir.is_empty() {
                        break;
                    }
                    dir = LibraryPaths::dir_of(dir);
                }
            }
        }
        counts
    }

    /// 按分类统计文件数（位置级：item 的每个库内位置各计一次）
    pub fn category_counts(&self) -> HashMap<String, usize> {
        let inner = read_inner!(self);
        let mut counts: HashMap<String, usize> = HashMap::new();
        for item in inner.by_hash.values() {
            let n = item.locations.iter().filter(|l| !l.in_trash()).count();
            if n == 0 {
                continue;
            }
            for c in &item.categories {
                *counts.entry(c.clone()).or_insert(0) += n;
            }
        }
        counts
    }

    /// 条件查询（位置级展开：同内容多位置各自成条）。锁内只做过滤与轻量排序键投影，排序在锁外进行（读写互堵不含 O(N log N) 排序）；
    /// DTO 只投影分页窗口——大库下避免为全部命中项克隆完整 DTO（UI 卡死的根因之一）。
    /// total_size 为过滤后全量（未分页）的字节数合计（各位置 size 之和）。
    /// 窗口 DTO 投影需二次读锁：排序快照与 DTO 快照之间可能隔一个写事件，UI 列表差一帧无碍（SSE 会推送对齐）
    pub fn query(&self, q: &ItemQuery) -> (Vec<ItemDto>, usize, i64) {
        let (mut keyed, total_size) = {
            let inner = read_inner!(self);
            let entries = filter_locations(&inner, q);
            let total_size = entries.iter().map(|(_, l)| l.size).sum();
            let keyed = entries
                .into_iter()
                .map(|(i, l)| (sort_key(i, l, q), i.id.clone(), l.path.clone()))
                .collect::<Vec<_>>();
            (keyed, total_size)
        };
        let total = keyed.len();
        sort_keyed(&mut keyed, q, |p| p.as_str());
        let offset = q.offset.max(0) as usize;
        let limit = q.limit.max(1) as usize;
        let inner = read_inner!(self);
        let dtos = keyed
            .into_iter()
            .skip(offset)
            .take(limit)
            .filter_map(|(_, id, path)| {
                let item = inner.by_hash.get(&id)?;
                let loc = item.locations.iter().find(|l| l.path == path)?;
                Some(item.to_dto_at(loc, q.in_trash))
            })
            .collect();
        (dtos, total, total_size)
    }

    /// 骨架查询：与 query 同过滤、同排序（含确定性次序），投影为 id/path/width/height/star，不分页
    pub fn query_skeleton(&self, q: &ItemQuery) -> (Vec<ItemSkeletonDto>, i64) {
        let (mut keyed, total_size) = {
            let inner = read_inner!(self);
            let entries = filter_locations(&inner, q);
            let total_size = entries.iter().map(|(_, l)| l.size).sum();
            let keyed = entries
                .into_iter()
                .map(|(i, l)| {
                    (
                        sort_key(i, l, q),
                        i.id.clone(),
                        ItemSkeletonDto {
                            id: i.id.clone(),
                            path: l.path.clone(),
                            width: i.width,
                            height: i.height,
                            star: i.star,
                        },
                    )
                })
                .collect::<Vec<_>>();
            (keyed, total_size)
        };
        sort_keyed(&mut keyed, q, |s: &ItemSkeletonDto| s.path.as_str());
        (keyed.into_iter().map(|(_, _, s)| s).collect(), total_size)
    }

    /// 范围内全部 item 的 hash 快照（宽高为 0 的项优先，修复时最先被处理）。
    /// folder 按库内位置前缀匹配；category/tag 与位置无关；library 含回收站
    pub fn hashes_in_scope(&self, scope: &RefreshScope) -> Vec<String> {
        let inner = read_inner!(self);
        let hashes = match scope {
            RefreshScope::Library => inner
                .by_hash
                .values()
                .map(|i| (i.id.clone(), i.width))
                .collect::<Vec<_>>(),
            RefreshScope::Folder(f) => inner
                .by_hash
                .values()
                .filter(|i| i.locations.iter().any(|l| !l.in_trash() && loc_in_folder(l, f, false)))
                .map(|i| (i.id.clone(), i.width))
                .collect::<Vec<_>>(),
            RefreshScope::Category(c) => inner
                .by_hash
                .values()
                .filter(|i| i.categories.contains(c))
                .map(|i| (i.id.clone(), i.width))
                .collect::<Vec<_>>(),
            RefreshScope::Tag(t) => inner
                .by_hash
                .values()
                .filter(|i| i.tags.contains(t))
                .map(|i| (i.id.clone(), i.width))
                .collect::<Vec<_>>(),
        };
        drop(inner);
        let mut hashes = hashes;
        hashes.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        hashes.into_iter().map(|(h, _)| h).collect()
    }
}

/// 过滤（AND 语义），位置级展开：同内容多位置各自成条。
/// 先按 item 级条件（ids/tags/star/categories/annotation/url/color 等元数据）粗筛，
/// 再展开该视图侧的位置做位置级条件（name 关键词/folders/ext）。返回引用，零克隆
fn filter_locations<'a>(inner: &'a IndexInner, q: &ItemQuery) -> Vec<(&'a Item, &'a ItemLocation)> {
    let mut items: Vec<&Item> = inner.by_hash.values().collect();

    if let Some(ids) = &q.ids {
        if !ids.is_empty() {
            let set: HashSet<&String> = ids.iter().collect();
            items.retain(|i| set.contains(&i.id));
        }
    }
    if let Some(tags) = &q.tags {
        if !tags.is_empty() {
            items.retain(|i| tags.iter().all(|t| i.tags.contains(t)));
        }
    }
    if let Some(star) = q.star {
        items.retain(|i| i.star == star);
    }
    if let Some(categories) = &q.categories {
        if !categories.is_empty() {
            let match_all = q.categories_match.as_deref().map(|m| m.eq_ignore_ascii_case("all")).unwrap_or(false);
            items.retain(|i| {
                if match_all {
                    categories.iter().all(|c| i.categories.contains(c))
                } else {
                    categories.iter().any(|c| i.categories.contains(c))
                }
            });
        }
    }
    if let Some(exclude) = &q.exclude_categories {
        if !exclude.is_empty() {
            items.retain(|i| !exclude.iter().any(|c| i.categories.contains(c)));
        }
    }
    if let Some(exclude) = &q.exclude_tags {
        if !exclude.is_empty() {
            items.retain(|i| !exclude.iter().any(|t| i.tags.contains(t)));
        }
    }
    if q.without_categories {
        items.retain(|i| i.categories.is_empty());
    }
    if q.without_tags {
        items.retain(|i| i.tags.is_empty());
    }
    if let Some(annotation) = &q.annotation {
        if !annotation.is_empty() {
            items.retain(|i| i.annotation.as_deref().map(|a| a.to_lowercase().contains(&annotation.to_lowercase())).unwrap_or(false));
        }
    }
    if let Some(url) = &q.url {
        if !url.is_empty() {
            items.retain(|i| i.url.as_deref().map(|u| u.to_lowercase().contains(&url.to_lowercase())).unwrap_or(false));
        }
    }
    if let Some(color) = q.color {
        items.retain(|i| i.palette.iter().any(|p| delta_e_squared(p.lab, color) <= COLOR_MATCH_THRESHOLD_SQUARED));
    }

    // 位置级展开：只保留本视图侧位置，再按位置属性过滤
    let mut entries: Vec<(&Item, &ItemLocation)> = Vec::new();
    for item in items {
        for loc in item.locations.iter().filter(|l| l.in_trash() == q.in_trash) {
            entries.push((item, loc));
        }
    }
    if let Some(keywords) = &q.keywords {
        if !keywords.is_empty() {
            entries.retain(|(i, l)| keywords.iter().all(|k| matches_keyword(i, l, k)));
        }
    }
    if let Some(folders) = &q.folders {
        if !folders.is_empty() {
            entries.retain(|(_, l)| folders.iter().any(|f| loc_in_folder(l, f, q.folders_exact)));
        }
    }
    if let Some(exclude) = &q.exclude_folders {
        if !exclude.is_empty() {
            // 子树整体剔除；空字符串（库根）命中一切，无排除语义，防御性跳过
            entries.retain(|(_, l)| !exclude.iter().any(|f| !f.is_empty() && loc_in_folder(l, f, false)));
        }
    }
    if let Some(ext) = &q.ext {
        if !ext.is_empty() {
            entries.retain(|(_, l)| LibraryPaths::ext_of(l.library_path()) == ext.to_lowercase());
        }
    }
    entries
}

/// 排序键：查询时锁内预计算（name 键预转小写避免比较器内反复分配；size/mtime/star 统一为数值键），
/// 排序在锁外进行。位置级：name/size/mtime 取该位置；star 为 item 级。同一查询键类型一致，不会混合
enum SortKey {
    Name(String),
    Num(i64),
}

fn sort_key(item: &Item, loc: &ItemLocation, q: &ItemQuery) -> SortKey {
    match q.order_by.as_deref().unwrap_or("modification_time") {
        "name" => SortKey::Name(LibraryPaths::name_of(loc.library_path()).to_lowercase()),
        "size" => SortKey::Num(loc.size),
        "star" => SortKey::Num(i64::from(item.star)),
        _ => SortKey::Num(loc.modification_time),
    }
}

fn cmp_sort_key(a: &SortKey, b: &SortKey) -> Ordering {
    match (a, b) {
        (SortKey::Name(x), SortKey::Name(y)) => x.cmp(y),
        (SortKey::Num(x), SortKey::Num(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

/// 锁外排序。主键同值时按 (id, path) 字典序打破平局：排序不稳定 + 两次独立查询（骨架/视口窗口）
/// 的次序必须逐位一致，否则按 offset 取窗口会错位；同内容多位置的排序键相同，须再按 path 决胜。
/// desc 反转整个比较结果（含平局决胜）
fn sort_keyed<T>(entries: &mut [(SortKey, String, T)], q: &ItemQuery, path_of: impl Fn(&T) -> &str) {
    let desc = !q.order.as_deref().map(|o| o.eq_ignore_ascii_case("asc")).unwrap_or(false);
    entries.sort_by(|a, b| {
        let c = cmp_sort_key(&a.0, &b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| path_of(&a.2).cmp(path_of(&b.2)));
        if desc { c.reverse() } else { c }
    });
}

fn matches_keyword(item: &Item, loc: &ItemLocation, keyword: &str) -> bool {
    if LibraryPaths::name_of(loc.library_path()).to_lowercase().contains(&keyword.to_lowercase()) {
        return true;
    }
    item.annotation
        .as_deref()
        .map(|a| a.to_lowercase().contains(&keyword.to_lowercase()))
        .unwrap_or(false)
}

/// 位置的文件夹匹配：默认前缀匹配（folder 本身及子目录；folder 为空串表示整个素材库）；
/// exact 时只匹配直接位于该目录下的位置（空串 = 库根目录，不含任何子文件夹）
fn loc_in_folder(loc: &ItemLocation, folder: &str, exact: bool) -> bool {
    let dir = LibraryPaths::dir_of(loc.library_path());
    if exact {
        dir == folder
    } else {
        folder.is_empty() || dir == folder || dir.starts_with(&format!("{folder}/"))
    }
}

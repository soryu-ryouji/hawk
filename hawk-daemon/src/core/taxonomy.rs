//! 分类/标签维度：名称校验、注册表（.hawk/categories.toml、.hawk/tags.toml，原子写）、
//! 级联迁移器（全库批迁移，只被索引流水线消费循环调用）。

use crate::core::events::EventBus;
use crate::core::index::ItemIndex;
use crate::core::item::ItemDto;
use crate::core::metadata::{toml_string, ItemMetadata};
use crate::core::metadata_store::MetadataStore;
use crate::core::paths::LibraryPaths;
use std::sync::RwLock;

/// 分类名称校验（扁平，无层级）：trim；空或含斜杠/反斜杠为非法
pub fn normalize_category_name(raw: Option<&str>) -> Option<String> {
    let name = raw?.trim();
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return None;
    }
    Some(name.to_string())
}

/// 注册表文件读写：固定 schema（key = [字符串数组]），原子写
fn load_registry(file: &str, key: &str) -> Vec<String> {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let value: toml::Value = match toml::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("注册表解析失败，按空表处理: {file}: {e}");
            return Vec::new();
        }
    };
    let mut out: Vec<String> = value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    out = out.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    out.dedup();
    out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    out
}

fn save_registry(file: &str, key: &str, entries: &[String]) {
    let body = format!(
        "{} = [{}]\n",
        key,
        entries.iter().map(|e| toml_string(e)).collect::<Vec<_>>().join(", ")
    );
    let tmp = format!("{file}.tmp");
    if std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// 分类/标签注册表共用骨架：持久化空名字（先建后放），写入只发生在索引流水线
struct Registry {
    file: String,
    key: &'static str,
    entries: RwLock<Vec<String>>,
}

impl Registry {
    fn new(file: String, key: &'static str) -> Registry {
        let entries = load_registry(&file, key);
        Registry {
            file,
            key,
            entries: RwLock::new(entries),
        }
    }

    fn snapshot(&self) -> Vec<String> {
        self.entries.read().unwrap().clone()
    }

    fn contains(&self, name: &str) -> bool {
        self.entries.read().unwrap().iter().any(|e| e == name)
    }

    fn register(&self, name: &str) {
        let mut entries = self.entries.write().unwrap();
        if !entries.iter().any(|e| e == name) {
            entries.push(name.to_string());
            entries.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
            save_registry(&self.file, self.key, &entries);
        }
    }

    fn register_all(&self, names: &[String]) {
        for name in names {
            self.register(name);
        }
    }

    /// 重命名；目标已存在时合并（集合语义）
    fn rename(&self, old_name: &str, new_name: &str) {
        let mut entries = self.entries.write().unwrap();
        if entries.iter().any(|e| e == old_name) {
            entries.retain(|e| e != old_name);
            if !entries.iter().any(|e| e == new_name) {
                entries.push(new_name.to_string());
            }
            entries.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
            save_registry(&self.file, self.key, &entries);
        }
    }

    fn delete(&self, name: &str) {
        let mut entries = self.entries.write().unwrap();
        if entries.iter().any(|e| e == name) {
            entries.retain(|e| e != name);
            save_registry(&self.file, self.key, &entries);
        }
    }

    fn reload(&self) {
        let mut entries = self.entries.write().unwrap();
        *entries = load_registry(&self.file, self.key);
    }
}

pub struct CategoryRegistry {
    inner: Registry,
}

impl CategoryRegistry {
    pub fn new(paths: &LibraryPaths) -> CategoryRegistry {
        CategoryRegistry {
            inner: Registry::new(paths.categories_file.clone(), "categories"),
        }
    }
    pub fn snapshot(&self) -> Vec<String> {
        self.inner.snapshot()
    }
    pub fn contains(&self, name: &str) -> bool {
        self.inner.contains(name)
    }
    pub fn register(&self, name: &str) {
        self.inner.register(name);
    }
    pub fn register_all(&self, names: &[String]) {
        self.inner.register_all(names);
    }
    pub fn rename(&self, old: &str, new: &str) {
        self.inner.rename(old, new);
    }
    pub fn delete(&self, name: &str) {
        self.inner.delete(name);
    }
    pub fn reload(&self) {
        self.inner.reload();
    }
}

pub struct TagRegistry {
    inner: Registry,
}

impl TagRegistry {
    pub fn new(paths: &LibraryPaths) -> TagRegistry {
        TagRegistry {
            inner: Registry::new(paths.tags_file.clone(), "tags"),
        }
    }
    pub fn snapshot(&self) -> Vec<String> {
        self.inner.snapshot()
    }
    pub fn contains(&self, name: &str) -> bool {
        self.inner.contains(name)
    }
    pub fn register(&self, name: &str) {
        self.inner.register(name);
    }
    pub fn register_all(&self, names: &[String]) {
        self.inner.register_all(names);
    }
    pub fn rename(&self, old: &str, new: &str) {
        self.inner.rename(old, new);
    }
    pub fn delete(&self, name: &str) {
        self.inner.delete(name);
    }
    pub fn reload(&self) {
        self.inner.reload();
    }
}

// ---------- SSE 事件发布辅助 ----------

pub struct ItemEvents;

impl ItemEvents {
    pub const ADDED: &'static str = "item.added";
    pub const UPDATED: &'static str = "item.updated";
    pub const TRASHED: &'static str = "item.trashed";
    pub const RESTORED: &'static str = "item.restored";
    pub const REMOVED: &'static str = "item.removed";
    pub const FOLDER_CHANGED: &'static str = "folder.changed";
    pub const TASK_PROGRESS: &'static str = "task.progress";
    /// 批量入库（扫描导入）合并事件，负载 `{ ids: [...] }`；客户端按「有新增」信号重载骨架即可。
    /// 与单条 item.added 互斥：同一入库只会走其一
    pub const ITEMS_ADDED: &'static str = "items.added";
    /// item.updated 的批量变体（调色板批量回写等），负载 `{ items: [...] }`
    pub const ITEMS_UPDATED: &'static str = "items.updated";

    /// item 内容/元数据变更事件,负载为完整 Item 对象(回收站视图按需投影)
    pub fn publish_changed(bus: &EventBus, item_dto: &ItemDto) {
        bus.publish(Self::UPDATED, serde_json::to_value(item_dto).unwrap());
    }

    /// item 失去一个位置后的事件:无剩余位置 → removed;只剩回收站 → trashed;否则 updated。
    /// 调用前索引已完成变更（位置已摘除）
    pub fn publish_location_loss(bus: &EventBus, index: &ItemIndex, hash: &str) {
        if !index.contains(hash) {
            bus.publish(Self::REMOVED, serde_json::json!({ "id": hash }));
        } else if !index.has_library_location(hash) {
            bus.publish(Self::TRASHED, serde_json::json!({ "id": hash }));
        } else if let Some(dto) = index.get_dto(hash) {
            bus.publish(Self::UPDATED, serde_json::to_value(&dto).unwrap());
        }
    }

    /// 位置进出回收站后的事件:首个库内位置进回收站 → trashed;首个回收站位置回归 → restored;其余 updated
    pub fn publish_transition(bus: &EventBus, index: &ItemIndex, hash: &str, was_in_trash: bool, now_in_trash: bool) {
        let library_count = index.library_location_count(hash);
        if !was_in_trash && now_in_trash && library_count == 0 {
            bus.publish(Self::TRASHED, serde_json::json!({ "id": hash }));
        } else if was_in_trash && !now_in_trash && library_count == 1 {
            if let Some(dto) = index.get_dto(hash) {
                bus.publish(Self::RESTORED, serde_json::to_value(&dto).unwrap());
            }
        } else if let Some(dto) = index.get_dto(hash) {
            bus.publish(Self::UPDATED, serde_json::to_value(&dto).unwrap());
        }
    }
}

/// 分类/标签级联迁移与元数据写应用。只被索引流水线的消费循环调用(单写者)
pub struct TaxonomyMigrator {
    store: std::sync::Arc<MetadataStore>,
    index: std::sync::Arc<ItemIndex>,
    categories: std::sync::Arc<CategoryRegistry>,
    tags: std::sync::Arc<TagRegistry>,
    bus: EventBus,
}

impl TaxonomyMigrator {
    pub fn new(
        store: std::sync::Arc<MetadataStore>,
        index: std::sync::Arc<ItemIndex>,
        categories: std::sync::Arc<CategoryRegistry>,
        tags: std::sync::Arc<TagRegistry>,
        bus: EventBus,
    ) -> TaxonomyMigrator {
        TaxonomyMigrator {
            store,
            index,
            categories,
            tags,
            bus,
        }
    }

    /// 元数据中的分类/标签自动登记进注册表(赋值即创建,空节点也可预创建)
    pub fn register_taxonomy(&self, meta: &ItemMetadata) {
        self.categories.register_all(&meta.categories);
        self.tags.register_all(&meta.tags);
    }

    pub fn register_category(&self, name: &str) {
        self.categories.register(name);
    }

    pub fn register_tag(&self, name: &str) {
        self.tags.register(name);
    }

    /// 注册表文件被外部修改(网盘同步等)时重载
    pub fn reload_registries(&self) {
        self.categories.reload();
        self.tags.reload();
    }

    /// MetadataJob 处理:应用变更 → 落盘 → 同步索引 → 发事件;元数据不存在返回 None
    pub fn apply_metadata(
        &self,
        hash: &str,
        mutate: impl FnOnce(&mut ItemMetadata),
    ) -> Result<Option<ItemDto>, String> {
        let mut meta = match self.store.try_get(hash) {
            Some(m) => m,
            None => return Ok(None),
        };
        mutate(&mut meta);
        self.store.save(hash, &meta)?;
        self.register_taxonomy(&meta);

        if self.index.contains(hash) {
            self.index.with_item_mut(hash, |item| item.sync_from(&meta));
            if let Some(dto) = self.index.get_dto(hash) {
                ItemEvents::publish_changed(&self.bus, &dto);
                return Ok(Some(dto));
            }
        }
        Ok(None)
    }

    /// 批量元数据应用(item/batch_update):逐个 mutate + 落盘 + 同步;
    /// 不存在的 id 记入 missing_ids(跳过),返回实际更新数。每个更新各发一个 item.updated
    pub fn apply_metadata_batch(
        &self,
        hashes: &[String],
        mutate: &mut dyn FnMut(&mut ItemMetadata),
        missing_ids: &mut Vec<String>,
    ) -> Result<usize, String> {
        let mut updated = 0;
        for hash in hashes {
            let mut meta = match self.store.try_get(hash) {
                Some(m) => m,
                None => {
                    missing_ids.push(hash.clone());
                    continue;
                }
            };
            mutate(&mut meta);
            self.store.save(hash, &meta)?;
            self.register_taxonomy(&meta);

            if self.index.contains(hash) {
                self.index.with_item_mut(hash, |item| item.sync_from(&meta));
                if let Some(dto) = self.index.get_dto(hash) {
                    ItemEvents::publish_changed(&self.bus, &dto);
                }
            }
            updated += 1;
        }
        Ok(updated)
    }

    /// 分类重命名:注册表更名 + 全部命中 item 的 categories 替换;目标已存在时合并
    pub fn rename_category(&self, old_name: &str, new_name: &str) -> Result<(), String> {
        self.categories.rename(old_name, new_name);
        // 分类可能仅由赋值产生而未注册过,补上登记
        self.categories.register(new_name);

        for (hash, mut meta) in self.store.snapshot() {
            if !meta.categories.iter().any(|c| c == old_name) {
                continue;
            }
            let replaced: Vec<String> = meta
                .categories
                .iter()
                .map(|c| if c == old_name { new_name.to_string() } else { c.clone() })
                .collect();
            meta.categories = dedup_preserve_order(&replaced);
            self.save_and_sync(&hash, &meta)?;
        }
        Ok(())
    }

    /// 分类删除:注册表与全部 item 的该分类赋值一并清除
    pub fn delete_category(&self, name: &str) -> Result<(), String> {
        self.categories.delete(name);
        for (hash, mut meta) in self.store.snapshot() {
            let before = meta.categories.len();
            meta.categories.retain(|c| c != name);
            if meta.categories.len() != before {
                self.save_and_sync(&hash, &meta)?;
            }
        }
        Ok(())
    }

    /// 标签重命名:注册表更名 + 全部 item 的 tags 替换;目标已存在时合并
    pub fn rename_tag(&self, name: &str, new_name: &str) -> Result<(), String> {
        self.tags.rename(name, new_name);
        for (hash, mut meta) in self.store.snapshot() {
            if !meta.tags.iter().any(|t| t == name) {
                continue;
            }
            meta.tags = dedup_preserve_order(&meta.tags.iter().map(|t| if t == name { new_name.to_string() } else { t.clone() }).collect::<Vec<_>>());
            self.save_and_sync(&hash, &meta)?;
        }
        Ok(())
    }

    /// 标签删除:注册表与全部 item 的该标签清除
    pub fn delete_tag(&self, name: &str) -> Result<(), String> {
        self.tags.delete(name);
        for (hash, mut meta) in self.store.snapshot() {
            let before = meta.tags.len();
            meta.tags.retain(|t| t != name);
            if meta.tags.len() != before {
                self.save_and_sync(&hash, &meta)?;
            }
        }
        Ok(())
    }

    /// 批量迁移的公共收尾:保存元数据、同步索引、推送 item.updated
    fn save_and_sync(&self, hash: &str, meta: &ItemMetadata) -> Result<(), String> {
        self.store.save(hash, meta)?;
        if self.index.contains(hash) {
            self.index.with_item_mut(hash, |item| item.sync_from(meta));
            if let Some(dto) = self.index.get_dto(hash) {
                ItemEvents::publish_changed(&self.bus, &dto);
            }
        }
        Ok(())
    }
}

fn dedup_preserve_order(items: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(item.clone()) {
            out.push(item.clone());
        }
    }
    out
}

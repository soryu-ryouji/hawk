namespace Hawk.Server.Core;

/// <summary>
/// 分类/标签级联迁移与元数据写应用。从 IndexPipeline 拆出;
/// 只被索引流水线的消费循环调用(单写者),所有写入经 MetadataStore 落盘并同步索引副本后发事件。
/// </summary>
public sealed class TaxonomyMigrator
{
    private readonly MetadataStore _store;
    private readonly ItemIndex _index;
    private readonly CategoryRegistry _categories;
    private readonly TagRegistry _tags;
    private readonly EventBus _bus;

    public TaxonomyMigrator(
        MetadataStore store,
        ItemIndex index,
        CategoryRegistry categories,
        TagRegistry tags,
        EventBus bus)
    {
        _store = store;
        _index = index;
        _categories = categories;
        _tags = tags;
        _bus = bus;
    }

    /// <summary>元数据中的分类/标签自动登记进注册表(赋值即创建,空节点也可预创建)</summary>
    public void RegisterTaxonomy(ItemMetadata meta)
    {
        _categories.RegisterAll(meta.Categories);
        _tags.RegisterAll(meta.Tags);
    }

    /// <summary>登记单个分类(空分类预创建)</summary>
    public void RegisterCategory(string name) => _categories.Register(name);

    /// <summary>登记单个标签(空标签预创建)</summary>
    public void RegisterTag(string name) => _tags.Register(name);

    /// <summary>注册表文件被外部修改(网盘同步等)时重载</summary>
    public void ReloadRegistries()
    {
        _categories.Reload();
        _tags.Reload();
    }

    /// <summary>MetadataJob 处理:应用变更 → 落盘 → 同步索引 → 发事件;元数据不存在返回 null</summary>
    public ItemDto? ApplyMetadata(string hash, Action<ItemMetadata> mutate)
    {
        if (!_store.TryGet(hash, out var meta))
        {
            return null;
        }

        mutate(meta);
        _store.Save(hash, meta);
        RegisterTaxonomy(meta);

        var item = _index.Get(hash);
        if (item is not null)
        {
            item.SyncFrom(meta);
            ItemEvents.PublishChanged(_bus, item);
            return item.ToDto(trashView: !item.HasLibraryLocations);
        }

        return null;
    }

    /// <summary>
    /// 批量元数据应用(item/batch_update):逐个 mutate + 落盘 + 同步;
    /// 不存在的 id 记入 missingIds(跳过),返回实际更新数。每个更新各发一个 item.updated。
    /// </summary>
    public int ApplyMetadataBatch(IReadOnlyList<string> hashes, Action<ItemMetadata> mutate, List<string> missingIds)
    {
        var updated = 0;
        foreach (var hash in hashes)
        {
            if (!_store.TryGet(hash, out var meta))
            {
                missingIds.Add(hash);
                continue;
            }

            mutate(meta);
            _store.Save(hash, meta);
            RegisterTaxonomy(meta);

            if (_index.Get(hash) is { } item)
            {
                item.SyncFrom(meta);
                ItemEvents.PublishChanged(_bus, item);
            }

            updated++;
        }

        return updated;
    }

    /// <summary>分类重命名:注册表更名 + 全部命中 item 的 categories 替换;目标已存在时合并</summary>
    public void RenameCategory(string oldName, string newName)
    {
        _categories.Rename(oldName, newName);
        // 分类可能仅由赋值产生而未注册过,补上登记
        _categories.Register(newName);

        foreach (var (hash, meta) in _store.Snapshot())
        {
            if (!meta.Categories.Contains(oldName))
            {
                continue;
            }

            meta.Categories = meta.Categories
                .Select(c => c == oldName ? newName : c)
                .Distinct(StringComparer.Ordinal)
                .ToList();
            SaveAndSync(hash, meta);
        }
    }

    /// <summary>分类删除:注册表与全部 item 的该分类赋值一并清除</summary>
    public void DeleteCategory(string name)
    {
        _categories.Delete(name);

        foreach (var (hash, meta) in _store.Snapshot())
        {
            if (meta.Categories.Remove(name))
            {
                SaveAndSync(hash, meta);
            }
        }
    }

    /// <summary>标签重命名:注册表更名 + 全部 item 的 tags 替换;目标已存在时合并</summary>
    public void RenameTag(string name, string newName)
    {
        _tags.Rename(name, newName);

        foreach (var (hash, meta) in _store.Snapshot())
        {
            if (!meta.Tags.Contains(name))
            {
                continue;
            }

            meta.Tags = meta.Tags.Select(t => t == name ? newName : t).Distinct(StringComparer.Ordinal).ToList();
            SaveAndSync(hash, meta);
        }
    }

    /// <summary>标签删除:注册表与全部 item 的该标签清除</summary>
    public void DeleteTag(string name)
    {
        _tags.Delete(name);

        foreach (var (hash, meta) in _store.Snapshot())
        {
            if (meta.Tags.RemoveAll(t => t == name) > 0)
            {
                SaveAndSync(hash, meta);
            }
        }
    }

    /// <summary>批量迁移的公共收尾:保存元数据、同步索引、推送 item.updated</summary>
    private void SaveAndSync(string hash, ItemMetadata meta)
    {
        _store.Save(hash, meta);
        if (_index.Get(hash) is { } item)
        {
            item.SyncFrom(meta);
            ItemEvents.PublishChanged(_bus, item);
        }
    }
}

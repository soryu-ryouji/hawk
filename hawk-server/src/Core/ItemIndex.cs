namespace Hawk.Server.Core;

/// <summary>item/list 的查询条件（全部可选，组合逻辑为 AND）</summary>
public sealed record ItemQuery
{
    public string[]? Ids { get; init; }
    public string[]? Keywords { get; init; }
    public string[]? Tags { get; init; }
    public int? Star { get; init; }
    public string[]? Folders { get; init; }
    public string[]? Categories { get; init; }
    public string? CategoriesMatch { get; init; }   // "any"（默认）/ "all"
    public string[]? ExcludeCategories { get; init; }
    public string[]? ExcludeTags { get; init; }
    public string? Ext { get; init; }
    public string? Annotation { get; init; }
    public string? Url { get; init; }
    public bool InTrash { get; init; }
    public string? OrderBy { get; init; }
    public string? Order { get; init; }
    public int Offset { get; init; }
    public int Limit { get; init; } = 50;
}

/// <summary>
/// 内存索引：hash → item，位置路径 → hash 反查。
/// 写入只发生在索引流水线（单写者），读取可来自任意 HTTP 线程，统一用一把锁保护。
/// </summary>
public sealed class ItemIndex
{
    private readonly object _gate = new();
    private readonly Dictionary<string, Item> _byHash = new();
    private readonly Dictionary<string, string> _hashByLocation = new(); // 位置路径 → hash

    public Item? Get(string hash)
    {
        lock (_gate)
        {
            return _byHash.GetValueOrDefault(hash);
        }
    }

    public string? HashByLocation(string locationPath)
    {
        lock (_gate)
        {
            return _hashByLocation.GetValueOrDefault(locationPath);
        }
    }

    /// <summary>库内 item 总数（不含回收站）</summary>
    public int Count()
    {
        lock (_gate)
        {
            return _byHash.Values.Count(i => i.HasLibraryLocations);
        }
    }

    /// <summary>全部位置路径快照（扫描时做消失检测用）</summary>
    public string[] AllLocationPaths()
    {
        lock (_gate)
        {
            return _hashByLocation.Keys.ToArray();
        }
    }

    /// <summary>取得或创建 item（不存在时创建并登记）</summary>
    public Item GetOrAdd(string hash, out bool created)
    {
        lock (_gate)
        {
            if (_byHash.TryGetValue(hash, out var item))
            {
                created = false;
                return item;
            }

            item = new Item { Id = hash };
            _byHash[hash] = item;
            created = true;
            return item;
        }
    }

    /// <summary>登记/刷新一个位置。返回是否为新增位置。</summary>
    public bool AddOrUpdateLocation(string hash, string locationPath, long size, long mtime)
    {
        lock (_gate)
        {
            var item = _byHash[hash];
            var loc = item.Locations.FirstOrDefault(l => l.Path == locationPath);
            if (loc is null)
            {
                item.Locations.Add(new ItemLocation { Path = locationPath, Size = size, ModificationTime = mtime });
                _hashByLocation[locationPath] = hash;
                return true;
            }

            loc.Size = size;
            loc.ModificationTime = mtime;
            return false;
        }
    }

    /// <summary>移除一个位置；item 不再有任何位置时从索引移除。返回受影响 item（可能已被移除）。</summary>
    public Item? RemoveLocation(string locationPath)
    {
        lock (_gate)
        {
            if (!_hashByLocation.Remove(locationPath, out var hash))
            {
                return null;
            }

            if (!_byHash.TryGetValue(hash, out var item))
            {
                return null;
            }

            item.Locations.RemoveAll(l => l.Path == locationPath);
            if (item.Locations.Count == 0)
            {
                _byHash.Remove(hash);
            }

            return item;
        }
    }

    /// <summary>位置移动（同内容改名/移动/进出回收站，hash 不变）。返回所属 hash，未索引返回 null。</summary>
    public string? MoveLocation(string oldPath, string newPath)
    {
        lock (_gate)
        {
            if (!_hashByLocation.Remove(oldPath, out var hash))
            {
                return null;
            }

            _hashByLocation[newPath] = hash;
            var loc = _byHash[hash].Locations.First(l => l.Path == oldPath);
            loc.Path = newPath;
            return hash;
        }
    }

    /// <summary>某目录前缀下的全部位置路径快照</summary>
    public string[] LocationsUnder(string relDirPrefix)
    {
        lock (_gate)
        {
            return _hashByLocation.Keys.Where(p => p.StartsWith(relDirPrefix, StringComparison.Ordinal)).ToArray();
        }
    }

    /// <summary>全部分类路径快照（含回收站 item 的赋值，分类树派生用）</summary>
    public string[] AllCategories()
    {
        lock (_gate)
        {
            return _byHash.Values.SelectMany(i => i.Categories).Distinct(StringComparer.Ordinal).ToArray();
        }
    }

    /// <summary>全部标签及库内计数快照（计数不含回收站）</summary>
    public (string Name, int Count)[] TagsWithCounts()
    {
        lock (_gate)
        {
            var counts = _byHash.Values.Where(i => i.HasLibraryLocations)
                .SelectMany(i => i.Tags)
                .GroupBy(t => t, StringComparer.Ordinal)
                .ToDictionary(g => g.Key, g => g.Count(), StringComparer.Ordinal);
            var names = _byHash.Values.SelectMany(i => i.Tags).ToHashSet(StringComparer.Ordinal);
            return names.Select(n => (n, counts.GetValueOrDefault(n))).ToArray();
        }
    }

    /// <summary>条件查询。在锁内完成过滤、排序、分页与 DTO 投影。</summary>
    public List<ItemDto> Query(ItemQuery q, out int total)
    {
        lock (_gate)
        {
            IEnumerable<Item> items = _byHash.Values.Where(i => q.InTrash ? i.HasTrashLocations : i.HasLibraryLocations);

            if (q.Ids is { Length: > 0 } ids)
            {
                var set = ids.ToHashSet();
                items = items.Where(i => set.Contains(i.Id));
            }

            if (q.Tags is { Length: > 0 } tags)
            {
                items = items.Where(i => tags.All(t => i.Tags.Contains(t, StringComparer.Ordinal)));
            }

            if (q.Star is { } star)
            {
                items = items.Where(i => i.Star == star);
            }

            if (q.Keywords is { Length: > 0 } keywords)
            {
                items = items.Where(i => keywords.All(k => MatchesKeyword(i, k, q.InTrash)));
            }

            if (q.Folders is { Length: > 0 } folders)
            {
                items = items.Where(i => folders.Any(f => InFolder(i, f, q.InTrash)));
            }

            if (q.Categories is { Length: > 0 } categories)
            {
                var matchAll = string.Equals(q.CategoriesMatch, "all", StringComparison.OrdinalIgnoreCase);
                items = items.Where(i => matchAll
                    ? categories.All(c => HasCategory(i, c))
                    : categories.Any(c => HasCategory(i, c)));
            }

            if (q.ExcludeCategories is { Length: > 0 } excludeCategories)
            {
                items = items.Where(i => !excludeCategories.Any(c => HasCategory(i, c)));
            }

            if (q.ExcludeTags is { Length: > 0 } excludeTags)
            {
                items = items.Where(i => !excludeTags.Any(t => i.Tags.Contains(t, StringComparer.Ordinal)));
            }

            if (!string.IsNullOrEmpty(q.Ext))
            {
                items = items.Where(i => MatchesExt(i, q.Ext, q.InTrash));
            }

            if (!string.IsNullOrEmpty(q.Annotation))
            {
                items = items.Where(i => i.Annotation?.Contains(q.Annotation, StringComparison.OrdinalIgnoreCase) == true);
            }

            if (!string.IsNullOrEmpty(q.Url))
            {
                items = items.Where(i => i.Url?.Contains(q.Url, StringComparison.OrdinalIgnoreCase) == true);
            }

            var dtos = items.Select(i => i.ToDto(q.InTrash)).ToList();
            total = dtos.Count;

            var desc = !string.Equals(q.Order, "asc", StringComparison.OrdinalIgnoreCase);
            dtos.Sort((a, b) =>
            {
                var c = (q.OrderBy ?? "modification_time") switch
                {
                    "name" => string.Compare(a.Name, b.Name, StringComparison.OrdinalIgnoreCase),
                    "size" => a.Size.CompareTo(b.Size),
                    "star" => a.Star.CompareTo(b.Star),
                    _ => a.ModificationTime.CompareTo(b.ModificationTime),
                };
                return desc ? -c : c;
            });

            return dtos.Skip(Math.Max(0, q.Offset)).Take(Math.Max(1, q.Limit)).ToList();
        }
    }

    private static bool HasCategory(Item item, string category) =>
        item.Categories.Any(c => CategoryPath.IsSameOrDescendant(c, category));

    private static bool MatchesKeyword(Item item, string keyword, bool trashView)
    {
        var main = item.MainLocation(trashView);
        if (main is not null && LibraryPaths.NameOf(main.LibraryPath).Contains(keyword, StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        return item.Annotation?.Contains(keyword, StringComparison.OrdinalIgnoreCase) == true;
    }

    private static bool InFolder(Item item, string folder, bool trashView) =>
        item.Locations.Any(l =>
        {
            if (l.InTrash != trashView)
            {
                return false;
            }

            var dir = LibraryPaths.DirOf(l.LibraryPath);
            return folder == "" || dir == folder || dir.StartsWith(folder + "/", StringComparison.Ordinal);
        });

    private static bool MatchesExt(Item item, string ext, bool trashView)
    {
        var main = item.MainLocation(trashView);
        return main is not null && string.Equals(LibraryPaths.ExtOf(main.LibraryPath), ext, StringComparison.OrdinalIgnoreCase);
    }
}

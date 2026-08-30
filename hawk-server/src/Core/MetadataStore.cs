using System.Collections;
using System.Text;
using Tomlyn;
using Tomlyn.Model;

namespace Hawk.Server.Core;

/// <summary>
/// 元数据存储：.hawk/metadata/&lt;hash&gt;.toml 的读写。.hawk/metadata/ 是唯一权威数据源，
/// 参与网盘同步；写入采用「临时文件 + rename」的原子写，避免网盘同步到写了一半的文件。
///
/// 本类内存中保留权威副本（含 path → hash 反查表）供流水线热路径查询；
/// 副本的注水来源优先级：SQLite 派生缓存（IndexDb，快路径）→ TOML 全量解析（回退+建缓存）。
/// 只有索引流水线写入本存储，因此内部状态用一把锁保护即可。
/// </summary>
public sealed class MetadataStore
{
    private readonly LibraryPaths _paths;
    private readonly IndexDb _db;
    private readonly ILogger<MetadataStore> _logger;
    private readonly object _gate = new();

    private readonly Dictionary<string, ItemMetadata> _byHash = new();
    private readonly Dictionary<string, string> _hashByPath = new(); // 库内路径 → hash

    public MetadataStore(LibraryPaths paths, IndexDb db, ILogger<MetadataStore> logger)
    {
        _paths = paths;
        _db = db;
        _logger = logger;

        List<(string Hash, ItemMetadata Meta, long SourceMtime)>? entries = null;
        if (_db.IsHydrated)
        {
            try
            {
                entries = _db.LoadAll();
                _logger.LogInformation("元数据副本已从 SQLite 缓存注水 {Count} 条", entries.Count);
            }
            catch (Exception ex)
            {
                _logger.LogError(ex, "元数据缓存读取失败，改由 TOML 全量解析");
            }
        }

        if (entries is null)
        {
            // 缓存缺失/未注水/读取失败：TOML 全量解析（一次性慢路径），并顺带建好缓存
            entries = LoadAllFromToml();
            _db.Hydrate(entries);
        }

        HydrateMemory(entries);
    }

    public bool TryGet(string hash, out ItemMetadata meta)
    {
        lock (_gate)
        {
            return _byHash.TryGetValue(hash, out meta!);
        }
    }

    /// <summary>全部元数据条目快照（批量迁移用）</summary>
    public KeyValuePair<string, ItemMetadata>[] Snapshot()
    {
        lock (_gate)
        {
            return _byHash.ToArray();
        }
    }

    /// <summary>按库内路径反查所属内容哈希（元数据 paths 记录）。</summary>
    public string? FindHashByPath(string libraryPath)
    {
        lock (_gate)
        {
            return _hashByPath.GetValueOrDefault(libraryPath);
        }
    }

    /// <summary>各 TOML 源文件 mtime 快照（后台对账比对依据）；缓存不可用时返回 null（本轮对账跳过）</summary>
    public IReadOnlyDictionary<string, long>? SourceMtimes() => _db.LoadSourceMtimes();

    /// <summary>
    /// 保存元数据：先 TOML 原子写（权威层），成功后更新内存副本与 SQLite 缓存。
    /// 中途崩溃时缓存与内存自然朝 TOML 收敛（后台对账会补齐），故必须先写 TOML。
    /// </summary>
    public void Save(string hash, ItemMetadata meta)
    {
        var file = FilePath(hash);
        var tmp = file + ".tmp";
        File.WriteAllText(tmp, Serialize(meta));
        File.Move(tmp, file, overwrite: true);
        var sourceMtime = LibraryPaths.ToUnixMs(File.GetLastWriteTimeUtc(file));

        lock (_gate)
        {
            _byHash[hash] = meta;
            RebuildPathIndex(hash, meta);
        }

        _db.Save(hash, meta, sourceMtime);
    }

    public void Delete(string hash)
    {
        var file = FilePath(hash);
        if (File.Exists(file))
        {
            File.Delete(file);
        }

        lock (_gate)
        {
            if (_byHash.Remove(hash, out var meta))
            {
                foreach (var p in meta.Paths)
                {
                    // 路径可能已迁移到其他 hash，只删仍指向本 hash 的映射
                    if (_hashByPath.TryGetValue(p.Path, out var owner) && owner == hash)
                    {
                        _hashByPath.Remove(p.Path);
                    }
                }
            }
        }

        _db.Delete(hash);
    }

    /// <summary>
    /// 对账应用（只进不出）：TOML 被外部新增/修改（网盘同步落地、手工编辑）后载入。
    /// 解析失败返回 false（跳过该文件，不清空任何状态，下一轮对账重试）。
    /// </summary>
    public bool ApplyExternalToml(string hash, string file, long sourceMtime)
    {
        ItemMetadata meta;
        try
        {
            meta = Parse(File.ReadAllText(file));
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "元数据解析失败，对账跳过: {File}", file);
            return false;
        }

        lock (_gate)
        {
            _byHash[hash] = meta;
            RebuildPathIndex(hash, meta);
        }

        _db.Save(hash, meta, sourceMtime);
        return true;
    }

    /// <summary>
    /// 对账应用（只进不出）：TOML 已消失 → 清空素材参数字段（等价于重启后无元数据的语义；
    /// item 本身与位置的存续由文件扫描决定，路径记录随之清空，扫描会按需重建空壳）。
    /// </summary>
    public void ClearExternal(string hash)
    {
        ItemMetadata meta;
        lock (_gate)
        {
            if (!_byHash.TryGetValue(hash, out meta!))
            {
                return;
            }

            meta.Url = null;
            meta.Star = 0;
            meta.Annotation = null;
            meta.Tags.Clear();
            meta.Categories.Clear();
            meta.Paths.Clear();
        }

        _db.Save(hash, meta, 0);
    }

    private string FilePath(string hash) => Path.Combine(_paths.MetadataDir, hash + ".toml");

    private void HydrateMemory(List<(string Hash, ItemMetadata Meta, long SourceMtime)> entries)
    {
        lock (_gate)
        {
            foreach (var (hash, meta, _) in entries)
            {
                _byHash[hash] = meta;
                RebuildPathIndex(hash, meta);
            }
        }
    }

    private void RebuildPathIndex(string hash, ItemMetadata meta)
    {
        foreach (var (path, h) in _hashByPath.Where(kv => kv.Value == hash).ToList())
        {
            _hashByPath.Remove(path);
        }

        foreach (var p in meta.Paths)
        {
            _hashByPath[p.Path] = hash;
        }
    }

    /// <summary>TOML 全量解析（缓存缺失时的权威回退路径）。只识别 64 位小写 hex 命名的文件，同步冲突副本等一律忽略。</summary>
    private List<(string Hash, ItemMetadata Meta, long SourceMtime)> LoadAllFromToml()
    {
        var entries = new List<(string, ItemMetadata, long)>();
        if (!Directory.Exists(_paths.MetadataDir))
        {
            return entries;
        }

        foreach (var file in Directory.EnumerateFiles(_paths.MetadataDir, "*.toml"))
        {
            var hash = Path.GetFileNameWithoutExtension(file);
            if (hash.Length != 64 || hash.Any(c => !char.IsAsciiHexDigitLower(c)))
            {
                continue;
            }

            try
            {
                var meta = Parse(File.ReadAllText(file));
                var mtime = LibraryPaths.ToUnixMs(File.GetLastWriteTimeUtc(file));
                entries.Add((hash, meta, mtime));
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "元数据解析失败，已跳过: {File}", file);
            }
        }

        _logger.LogInformation("已从 TOML 全量解析 {Count} 条元数据", entries.Count);
        return entries;
    }

    private static ItemMetadata Parse(string toml)
    {
        var table = TomlSerializer.Deserialize<TomlTable>(toml) ?? new TomlTable();
        var meta = new ItemMetadata();

        if (table.TryGetValue("url", out var url) && url is string urlStr)
        {
            meta.Url = urlStr;
        }

        if (table.TryGetValue("star", out var star) && star is long starNum)
        {
            meta.Star = (int)starNum;
        }

        if (table.TryGetValue("annotation", out var ann) && ann is string annStr)
        {
            meta.Annotation = annStr;
        }

        if (table.TryGetValue("tags", out var tags) && tags is IEnumerable tagArr)
        {
            meta.Tags = tagArr.Cast<object?>().OfType<string>().ToList();
        }

        if (table.TryGetValue("categories", out var cats) && cats is IEnumerable catArr)
        {
            meta.Categories = catArr.Cast<object?>().OfType<string>().ToList();
        }

        if (table.TryGetValue("paths", out var paths) && paths is IEnumerable pathArr)
        {
            foreach (var entry in pathArr.Cast<object?>())
            {
                if (entry is not IDictionary<string, object?> e || e["path"] is not string p)
                {
                    continue;
                }

                meta.Paths.Add(new PathEntry
                {
                    Path = p,
                    Size = e.TryGetValue("size", out var s) ? Convert.ToInt64(s) : 0,
                    ModificationTime = e.TryGetValue("modification_time", out var m) ? Convert.ToInt64(m) : 0,
                });
            }
        }

        return meta;
    }

    /// <summary>
    /// 序列化为 TOML。schema 固定，手写序列化以精确控制输出格式
    /// （标量在前、[[paths]] 在后，缺省字段省略）。
    /// </summary>
    private static string Serialize(ItemMetadata meta)
    {
        var sb = new StringBuilder();

        if (meta.Url is not null)
        {
            sb.Append("url = ").AppendLine(TomlString(meta.Url));
        }

        if (meta.Tags.Count > 0)
        {
            sb.Append("tags = [").Append(string.Join(", ", meta.Tags.Select(TomlString))).AppendLine("]");
        }

        if (meta.Categories.Count > 0)
        {
            sb.Append("categories = [").Append(string.Join(", ", meta.Categories.Select(TomlString))).AppendLine("]");
        }

        if (meta.Star > 0)
        {
            sb.Append("star = ").AppendLine(meta.Star.ToString());
        }

        if (meta.Annotation is not null)
        {
            sb.Append("annotation = ").AppendLine(TomlString(meta.Annotation));
        }

        foreach (var p in meta.Paths)
        {
            sb.AppendLine();
            sb.AppendLine("[[paths]]");
            sb.Append("path = ").AppendLine(TomlString(p.Path));
            sb.Append("size = ").AppendLine(p.Size.ToString());
            sb.Append("modification_time = ").AppendLine(p.ModificationTime.ToString());
        }

        return sb.ToString();
    }

    private static string TomlString(string value)
    {
        var sb = new StringBuilder(value.Length + 2);
        sb.Append('"');
        foreach (var c in value)
        {
            sb.Append(c switch
            {
                '\\' => "\\\\",
                '"' => "\\\"",
                '\n' => "\\n",
                '\r' => "\\r",
                '\t' => "\\t",
                _ when char.IsControl(c) => $"\\u{(int)c:X4}",
                _ => c,
            });
        }

        sb.Append('"');
        return sb.ToString();
    }
}

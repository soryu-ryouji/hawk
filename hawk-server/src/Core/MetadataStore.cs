using System.Collections;
using System.Text;
using Tomlyn;
using Tomlyn.Model;

namespace Hawk.Server.Core;

/// <summary>
/// 元数据存储：.hawk/metadata/&lt;hash&gt;.toml 的读写。
/// 内存中保留权威副本（含 path → hash 反查表），磁盘为持久化层；
/// 写入采用「临时文件 + rename」的原子写，避免网盘同步到写了一半的文件。
/// 只有索引流水线写入本存储，因此内部状态用一把锁保护即可。
/// </summary>
public sealed class MetadataStore
{
    private readonly LibraryPaths _paths;
    private readonly ILogger<MetadataStore> _logger;
    private readonly object _gate = new();

    private readonly Dictionary<string, ItemMetadata> _byHash = new();
    private readonly Dictionary<string, string> _hashByPath = new(); // 库内路径 → hash

    public MetadataStore(LibraryPaths paths, ILogger<MetadataStore> logger)
    {
        _paths = paths;
        _logger = logger;
        LoadAll();
    }

    public bool TryGet(string hash, out ItemMetadata meta)
    {
        lock (_gate)
        {
            return _byHash.TryGetValue(hash, out meta!);
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

    /// <summary>保存元数据（内存 + 磁盘原子写）。</summary>
    public void Save(string hash, ItemMetadata meta)
    {
        lock (_gate)
        {
            _byHash[hash] = meta;
            RebuildPathIndex(hash, meta);
        }

        var file = FilePath(hash);
        var tmp = file + ".tmp";
        File.WriteAllText(tmp, Serialize(meta));
        File.Move(tmp, file, overwrite: true);
    }

    public void Delete(string hash)
    {
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

        var file = FilePath(hash);
        if (File.Exists(file))
        {
            File.Delete(file);
        }
    }

    private string FilePath(string hash) => Path.Combine(_paths.MetadataDir, hash + ".toml");

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

    /// <summary>加载全部元数据。只识别 64 位小写 hex 命名的文件，同步冲突副本等一律忽略。</summary>
    private void LoadAll()
    {
        if (!Directory.Exists(_paths.MetadataDir))
        {
            return;
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
                lock (_gate)
                {
                    _byHash[hash] = meta;
                    RebuildPathIndex(hash, meta);
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "元数据解析失败，已跳过: {File}", file);
            }
        }

        _logger.LogInformation("已加载 {Count} 条元数据", _byHash.Count);
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

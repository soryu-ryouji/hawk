using System.Text;
using Tomlyn;
using Tomlyn.Model;

namespace Hawk.Server.Core;

/// <summary>分类路径（正斜杠分隔层级）的规范化与工具</summary>
public static class CategoryPath
{
    /// <summary>规范化：段 trim、去空段；非法（/./../反斜杠/全空）返回 null</summary>
    public static string? Normalize(string? raw)
    {
        if (string.IsNullOrWhiteSpace(raw))
        {
            return null;
        }

        var segments = raw.Split('/', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);
        if (segments.Length == 0 || segments.Any(s => s is "." or ".." || s.Contains('\\')))
        {
            return null;
        }

        return string.Join('/', segments);
    }

    /// <summary>父路径；根级分类返回 ""</summary>
    public static string ParentOf(string path)
    {
        var idx = path.LastIndexOf('/');
        return idx < 0 ? "" : path[..idx];
    }

    /// <summary>最后一段名称</summary>
    public static string NameOf(string path) => path[(path.LastIndexOf('/') + 1)..];

    /// <summary>path 是 prefix 自身或其后代</summary>
    public static bool IsSameOrDescendant(string path, string prefix) =>
        path == prefix || path.StartsWith(prefix + "/", StringComparison.Ordinal);
}

/// <summary>
/// 分类注册表（.hawk/categories.toml）：持久化空分类，支持「先建后放」。
/// 内容只是名字列表，网盘同步冲突代价低。树由路径派生，不单独存储结构。
/// 写入只发生在索引流水线；外部修改由文件监听触发 Reload。
/// </summary>
public sealed class CategoryRegistry
{
    private readonly object _gate = new();
    private readonly List<string> _entries = new();
    private readonly string _file;
    private readonly ILogger _logger;

    public CategoryRegistry(LibraryPaths paths, ILogger<CategoryRegistry> logger)
    {
        _file = paths.CategoriesFile;
        _logger = logger;
        Reload();
    }

    public string[] Snapshot()
    {
        lock (_gate)
        {
            return _entries.ToArray();
        }
    }

    public bool Contains(string path) => Snapshot().Contains(path, StringComparer.Ordinal);

    /// <summary>登记分类（含祖先补齐）；已存在则无动作</summary>
    public void Register(string path)
    {
        lock (_gate)
        {
            var changed = false;
            for (var p = path; p != ""; p = CategoryPath.ParentOf(p))
            {
                if (!_entries.Contains(p))
                {
                    _entries.Add(p);
                    changed = true;
                }
            }

            if (changed)
            {
                _entries.Sort(StringComparer.OrdinalIgnoreCase);
                SaveLocked();
            }
        }
    }

    public void RegisterAll(IEnumerable<string> paths)
    {
        foreach (var path in paths)
        {
            Register(path);
        }
    }

    /// <summary>重命名/移动：前缀迁移，子树跟随</summary>
    public void Rename(string oldPath, string newPath)
    {
        lock (_gate)
        {
            var changed = false;
            for (var i = 0; i < _entries.Count; i++)
            {
                if (CategoryPath.IsSameOrDescendant(_entries[i], oldPath))
                {
                    _entries[i] = newPath + _entries[i][oldPath.Length..];
                    changed = true;
                }
            }

            if (changed)
            {
                _entries.Sort(StringComparer.OrdinalIgnoreCase);
                SaveLocked();
            }
        }
    }

    /// <summary>删除节点及子树</summary>
    public void Delete(string path)
    {
        lock (_gate)
        {
            if (_entries.RemoveAll(e => CategoryPath.IsSameOrDescendant(e, path)) > 0)
            {
                SaveLocked();
            }
        }
    }

    public void Reload()
    {
        lock (_gate)
        {
            _entries.Clear();
            _entries.AddRange(TaxonomyFile.Load(_file, "categories", _logger));
            _entries.Sort(StringComparer.OrdinalIgnoreCase);
        }
    }

    private void SaveLocked() => TaxonomyFile.Save(_file, "categories", _entries);
}

/// <summary>标签注册表（.hawk/tags.toml）：扁平名字列表，支持空标签预创建</summary>
public sealed class TagRegistry
{
    private readonly object _gate = new();
    private readonly List<string> _entries = new();
    private readonly string _file;
    private readonly ILogger _logger;

    public TagRegistry(LibraryPaths paths, ILogger<TagRegistry> logger)
    {
        _file = paths.TagsFile;
        _logger = logger;
        Reload();
    }

    public string[] Snapshot()
    {
        lock (_gate)
        {
            return _entries.ToArray();
        }
    }

    public bool Contains(string name) => Snapshot().Contains(name, StringComparer.Ordinal);

    public void Register(string name)
    {
        lock (_gate)
        {
            if (!_entries.Contains(name))
            {
                _entries.Add(name);
                _entries.Sort(StringComparer.OrdinalIgnoreCase);
                SaveLocked();
            }
        }
    }

    public void RegisterAll(IEnumerable<string> names)
    {
        foreach (var name in names)
        {
            Register(name);
        }
    }

    /// <summary>重命名；目标已存在时合并（标签是集合语义）</summary>
    public void Rename(string oldName, string newName)
    {
        lock (_gate)
        {
            if (_entries.Remove(oldName))
            {
                if (!_entries.Contains(newName))
                {
                    _entries.Add(newName);
                }

                _entries.Sort(StringComparer.OrdinalIgnoreCase);
                SaveLocked();
            }
        }
    }

    public void Delete(string name)
    {
        lock (_gate)
        {
            if (_entries.Remove(name))
            {
                SaveLocked();
            }
        }
    }

    public void Reload()
    {
        lock (_gate)
        {
            _entries.Clear();
            _entries.AddRange(TaxonomyFile.Load(_file, "tags", _logger));
            _entries.Sort(StringComparer.OrdinalIgnoreCase);
        }
    }

    private void SaveLocked() => TaxonomyFile.Save(_file, "tags", _entries);
}

/// <summary>注册表文件读写：固定 schema（key = [字符串数组]），原子写</summary>
file static class TaxonomyFile
{
    public static List<string> Load(string file, string key, ILogger logger)
    {
        if (!File.Exists(file))
        {
            return [];
        }

        try
        {
            var table = TomlSerializer.Deserialize<TomlTable>(File.ReadAllText(file));
            if (table?.TryGetValue(key, out var value) == true && value is System.Collections.IEnumerable arr)
            {
                return arr.Cast<object?>().OfType<string>()
                    .Select(s => s.Trim())
                    .Where(s => s != "")
                    .Distinct(StringComparer.Ordinal)
                    .ToList();
            }
        }
        catch (Exception ex)
        {
            logger.LogWarning(ex, "注册表解析失败，按空表处理: {File}", file);
        }

        return [];
    }

    public static void Save(string file, string key, List<string> entries)
    {
        var sb = new StringBuilder();
        sb.Append(key).Append(" = [");
        sb.Append(string.Join(", ", entries.Select(Escape)));
        sb.AppendLine("]");

        var tmp = file + ".tmp";
        File.WriteAllText(tmp, sb.ToString());
        File.Move(tmp, file, overwrite: true);
    }

    private static string Escape(string value)
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

        return sb.Append('"').ToString();
    }
}

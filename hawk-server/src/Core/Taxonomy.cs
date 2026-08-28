using System.Text;
using Tomlyn;
using Tomlyn.Model;

namespace Hawk.Server.Core;

/// <summary>分类名称校验（扁平，无层级）：trim；空或含斜杠/反斜杠为非法</summary>
public static class CategoryName
{
    public static string? Normalize(string? raw)
    {
        if (string.IsNullOrWhiteSpace(raw))
        {
            return null;
        }

        var name = raw.Trim();
        return name.Contains('/') || name.Contains('\\') ? null : name;
    }
}

/// <summary>
/// 分类注册表（.hawk/categories.toml）：持久化空分类，支持「先建后放」。
/// 分类是扁平名字（item 可同时挂多个），与标签同构。写入只发生在索引流水线；外部修改由文件监听触发 Reload。
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

    public bool Contains(string name) => Snapshot().Contains(name, StringComparer.Ordinal);

    /// <summary>登记分类；已存在则无动作</summary>
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

    /// <summary>重命名；目标已存在时合并（分类是集合语义）</summary>
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

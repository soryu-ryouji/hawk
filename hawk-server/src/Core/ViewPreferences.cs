using System.Text;
using Tomlyn;
using Tomlyn.Model;

namespace Hawk.Server.Core;

/// <summary>一条排序偏好（item/list 的 order_by/order 参数白名单值）</summary>
public sealed record ViewSort(string OrderBy, string Order);

/// <summary>
/// 视图偏好注册表（.hawk/view.toml）：记住文件夹/分类/标签视图各自的排序方式。
///
/// 条目为扁平 map，scope 键三种形态：
/// - "folder:\u003c库内路径\u003e"（路径 "" 为库根）——继承由前端解析（沿父链向上，子文件夹自己的设置优先）
/// - "category:\u003c名称\u003e" / "tag:\u003c名称\u003e"（无层级，直接回落全局默认）
///
/// 本类只做存取与校验，不理解继承语义。写入直接进行（偏好与索引/元数据无耦合，
/// 自带锁保护）；外部修改（含网盘同步落地）由文件监听触发 Reload，语义与分类/标签注册表一致。
/// 文件夹移动/重命名/删除时由索引流水线调用 RenamePrefix/DeletePrefix 跟随清理。
/// </summary>
public sealed class ViewPreferences
{
    private static readonly string[] OrderByWhitelist = ["modification_time", "name", "size", "star"];
    private static readonly string[] OrderWhitelist = ["asc", "desc"];

    private readonly object _gate = new();
    private readonly Dictionary<string, ViewSort> _entries = new(StringComparer.Ordinal);
    private readonly string _file;
    private readonly ILogger _logger;

    public ViewPreferences(LibraryPaths paths, ILogger<ViewPreferences> logger)
    {
        _file = paths.ViewFile;
        _logger = logger;
        Reload();
    }

    public IReadOnlyDictionary<string, ViewSort> Snapshot()
    {
        lock (_gate)
        {
            return new Dictionary<string, ViewSort>(_entries, StringComparer.Ordinal);
        }
    }

    /// <summary>覆盖写一条偏好；scope 与 sort 须已过 <see cref="TryParseScope"/>/校验</summary>
    public void Set(string scope, ViewSort sort)
    {
        lock (_gate)
        {
            _entries[scope] = sort;
            SaveLocked();
        }
    }

    /// <summary>删除一条偏好（回到继承/默认）。不存在则无动作</summary>
    public void Delete(string scope)
    {
        lock (_gate)
        {
            if (_entries.Remove(scope))
            {
                SaveLocked();
            }
        }
    }

    /// <summary>文件夹移动/重命名：前缀范围内的 folder: 键跟随迁移</summary>
    public void RenamePrefix(string oldDir, string newDir)
    {
        lock (_gate)
        {
            var prefix = "folder:" + oldDir;
            var hits = _entries.Keys
                .Where(k => k == prefix || k.StartsWith(prefix + "/", StringComparison.Ordinal))
                .ToArray();
            if (hits.Length == 0)
            {
                return;
            }

            foreach (var key in hits)
            {
                var sort = _entries[key];
                _entries.Remove(key);
                _entries["folder:" + newDir + key[prefix.Length..]] = sort;
            }

            SaveLocked();
        }
    }

    /// <summary>文件夹删除：前缀范围内的 folder: 键一并清除（回收站移动不走这里，由 RenamePrefix 跟随）</summary>
    public void DeletePrefix(string dir)
    {
        lock (_gate)
        {
            var prefix = "folder:" + dir;
            var hits = _entries.Keys
                .Where(k => k == prefix || k.StartsWith(prefix + "/", StringComparison.Ordinal))
                .ToArray();
            if (hits.Length == 0)
            {
                return;
            }

            foreach (var key in hits)
            {
                _entries.Remove(key);
            }

            SaveLocked();
        }
    }

    /// <summary>外部修改（含网盘同步落地）后重载；解析失败的条目跳过</summary>
    public void Reload()
    {
        lock (_gate)
        {
            _entries.Clear();
            if (!File.Exists(_file))
            {
                return;
            }

            try
            {
                var table = TomlSerializer.Deserialize<TomlTable>(File.ReadAllText(_file));
                if (table is null)
                {
                    return;
                }

                foreach (var (key, value) in table)
                {
                    if (value is not IDictionary<string, object?> section)
                    {
                        continue;
                    }

                    if (TryParseScope(key, out var scope)
                        && section.TryGetValue("order_by", out var ob) && ob is string orderBy
                        && section.TryGetValue("order", out var o) && o is string order
                        && TryNormalizeSort(orderBy, order, out var sort))
                    {
                        _entries[scope] = sort;
                    }
                    else
                    {
                        _logger.LogWarning("视图偏好条目无效，已跳过: {Key}", key);
                    }
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "视图偏好解析失败，按空表处理: {File}", _file);
                _entries.Clear();
            }
        }
    }

    /// <summary>
    /// 校验并规范化 scope 键。folder 路径须为合法库内路径（"" 为库根）；
    /// category/tag 名称复用各自注册表的名称规则。
    /// </summary>
    public static bool TryParseScope(string raw, out string scope)
    {
        scope = "";
        if (raw.StartsWith("folder:", StringComparison.Ordinal))
        {
            var path = raw["folder:".Length..];
            if (path != "" && !LibraryPaths.IsValidLibraryPath(path))
            {
                return false;
            }

            scope = "folder:" + path;
            return true;
        }

        if (raw.StartsWith("category:", StringComparison.Ordinal))
        {
            var name = CategoryName.Normalize(raw["category:".Length..]);
            if (name is null)
            {
                return false;
            }

            scope = "category:" + name;
            return true;
        }

        if (raw.StartsWith("tag:", StringComparison.Ordinal))
        {
            var name = raw["tag:".Length..].Trim();
            if (name == "")
            {
                return false;
            }

            scope = "tag:" + name;
            return true;
        }

        return false;
    }

    /// <summary>校验并规范化排序值（order_by/order 白名单，小写）</summary>
    public static bool TryNormalizeSort(string orderBy, string order, out ViewSort sort)
    {
        sort = null!;
        var ob = orderBy.Trim().ToLowerInvariant();
        var o = order.Trim().ToLowerInvariant();
        if (!OrderByWhitelist.Contains(ob, StringComparer.Ordinal) || !OrderWhitelist.Contains(o, StringComparer.Ordinal))
        {
            return false;
        }

        sort = new ViewSort(ob, o);
        return true;
    }

    private void SaveLocked()
    {
        var sb = new StringBuilder();
        foreach (var (scope, sort) in _entries.OrderBy(e => e.Key, StringComparer.Ordinal))
        {
            sb.Append('[').Append(Escape(scope)).AppendLine("]");
            sb.Append("order_by = ").AppendLine(Escape(sort.OrderBy));
            sb.Append("order = ").AppendLine(Escape(sort.Order));
            sb.AppendLine();
        }

        var tmp = _file + ".tmp";
        File.WriteAllText(tmp, sb.ToString());
        File.Move(tmp, _file, overwrite: true);
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

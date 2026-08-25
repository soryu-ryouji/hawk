using System.Collections;
using Microsoft.Extensions.FileSystemGlobbing;
using Tomlyn;
using Tomlyn.Model;

namespace Hawk.Server.Core;

/// <summary>
/// 项目配置（.hawk/config.toml）。由文件监听触发 Reload，索引流水线在配置变更后全量比对。
/// 保存不可变快照，读取方通过 Current 获取，无需加锁。
/// </summary>
public sealed class LibraryConfig
{
    public sealed record Snapshot(string? Name, IReadOnlyList<string> Ignore, IReadOnlyList<int> ThumbnailSizes, Matcher IgnoreMatcher);

    private readonly LibraryPaths _paths;
    private readonly ILogger<LibraryConfig> _logger;
    private volatile Snapshot _current;

    public static readonly IReadOnlyList<int> DefaultThumbnailSizes = [256, 1024];

    public Snapshot Current => _current;

    public LibraryConfig(LibraryPaths paths, ILogger<LibraryConfig> logger)
    {
        _paths = paths;
        _logger = logger;
        _current = Load();
    }

    public void Reload() => _current = Load();

    /// <summary>相对路径是否被 ignore 规则命中（仅用于库内文件，回收站不参与）</summary>
    public bool IsIgnored(string relPath) => _current.IgnoreMatcher.Match(relPath).HasMatches;

    private Snapshot Load()
    {
        string? name = null;
        var ignore = new List<string>();
        IReadOnlyList<int> thumbnailSizes = DefaultThumbnailSizes;

        if (File.Exists(_paths.ConfigFile))
        {
            try
            {
                var table = TomlSerializer.Deserialize<TomlTable>(File.ReadAllText(_paths.ConfigFile)) ?? new TomlTable();
                name = table.TryGetValue("name", out var n) ? n as string : null;

                if (table.TryGetValue("ignore", out var ig) && ig is IEnumerable igArr)
                {
                    ignore.AddRange(igArr.Cast<object?>().OfType<string>());
                }

                if (table.TryGetValue("thumbnail_sizes", out var ts) && ts is IEnumerable tsArr)
                {
                    var sizes = tsArr.Cast<object?>().Select(Convert.ToInt32).Where(s => s > 0).ToArray();
                    if (sizes.Length > 0)
                    {
                        thumbnailSizes = sizes;
                    }
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "解析 config.toml 失败，使用默认配置");
            }
        }

        return new Snapshot(name, ignore, thumbnailSizes, BuildMatcher(ignore));
    }

    /// <summary>
    /// 构建 ignore 匹配器。不含 "/" 的模式匹配任意深度的同名文件/目录（如 node_modules、*.tmp）；
    /// 含 "/" 的模式按相对库根目录的路径匹配。
    /// 每个模式同时注册 <模式>/** 变体：目录模式需连同其内容一起排除（监听事件以文件路径到达）。
    /// </summary>
    private static Matcher BuildMatcher(IEnumerable<string> patterns)
    {
        var matcher = new Matcher();
        foreach (var pattern in patterns)
        {
            if (string.IsNullOrWhiteSpace(pattern))
            {
                continue;
            }

            var p = pattern.Contains('/') ? pattern : $"**/{pattern}";
            matcher.AddInclude(p);
            matcher.AddInclude(p + "/**");
        }

        return matcher;
    }
}

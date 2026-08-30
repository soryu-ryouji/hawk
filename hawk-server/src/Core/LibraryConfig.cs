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
    /// <summary>局域网 web 查看配置（config.toml 的 [web] 段，按库隔离）。
    /// 读取热更（文件监听 Reload），但端口/绑定/token 的生效需重启监听——保存后由 Electron 重启 server</summary>
    public sealed record WebSettings(bool Enabled, int Port, string? Token);

    public static readonly WebSettings DefaultWeb = new(false, 27372, null);

    public sealed record Snapshot(string? Name, IReadOnlyList<string> Ignore, IReadOnlyList<int> ThumbnailSizes, Matcher IgnoreMatcher, WebSettings Web);

    private readonly LibraryPaths _paths;
    private readonly ILogger<LibraryConfig> _logger;
    private volatile Snapshot _current;

    public static readonly IReadOnlyList<int> DefaultThumbnailSizes = [256, 512, 1024];

    public Snapshot Current => _current;

    public LibraryConfig(LibraryPaths paths, ILogger<LibraryConfig> logger)
    {
        _paths = paths;
        _logger = logger;
        EnsureDefaultConfig();
        _current = Load();
    }

    /// <summary>
    /// 库首次打开时生成带注释的默认 config.toml（已存在则不覆盖），让配置可发现、可手工编辑。
    /// 时机在构造期（监听启动前），生成不触发配置变更事件。
    /// </summary>
    private void EnsureDefaultConfig()
    {
        try
        {
            if (File.Exists(_paths.ConfigFile))
            {
                return;
            }

            Directory.CreateDirectory(Path.GetDirectoryName(_paths.ConfigFile)!);
            File.WriteAllText(_paths.ConfigFile, DefaultConfigText);
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "生成默认 config.toml 失败（不影响运行，使用默认配置）");
        }
    }

    private const string DefaultConfigText =
        """
        # hawk 项目配置（.hawk/config.toml，按素材库隔离、随库同步）
        # name / ignore / thumbnail_sizes 保存即热更（文件监听 Reload）
        # [web] 段的端口/绑定/token 保存后由桌面端重启服务生效

        # 素材库显示名（缺省为库目录名）
        # name = "我的素材库"

        # 索引时忽略的路径（不含 "/" 的模式匹配任意深度同名项）
        ignore = []

        # 生成的缩略图尺寸
        thumbnail_sizes = [256, 512, 1024]

        # 局域网 web 查看（只读；桌面端设置面板读写）
        [web]
        enabled = false
        port = 27372
        token = ""
        """;

    public void Reload() => _current = Load();

    /// <summary>启动期静态读取 [web] 段（Program.BuildUrls 在 DI 构建前调用）；运行时读取走 Current.Web</summary>
    public static WebSettings PeekWeb(string libraryRoot)
    {
        var file = Path.Combine(libraryRoot, ".hawk", "config.toml");
        return File.Exists(file) ? ParseWeb(file) : DefaultWeb;
    }

    /// <summary>解析 config.toml 的 [web] 段；缺段/非法值回退默认</summary>
    public static WebSettings ParseWeb(string configFile)
    {
        try
        {
            var table = TomlSerializer.Deserialize<TomlTable>(File.ReadAllText(configFile)) ?? new TomlTable();
            if (table.TryGetValue("web", out var w) && w is TomlTable webTable)
            {
                var enabled = webTable.TryGetValue("enabled", out var e) && e is bool b && b;
                var port = webTable.TryGetValue("port", out var p) && p is long l && l is > 0 and <= 65535 ? (int)l : DefaultWeb.Port;
                var token = webTable.TryGetValue("token", out var t) && t is string s && !string.IsNullOrWhiteSpace(s) ? s.Trim() : null;
                return new WebSettings(enabled, port, token);
            }
        }
        catch
        {
            // 解析失败按未配置处理
        }

        return DefaultWeb;
    }

    /// <summary>相对路径是否被 ignore 规则命中（仅用于库内文件，回收站不参与）</summary>
    public bool IsIgnored(string relPath) => _current.IgnoreMatcher.Match(relPath).HasMatches;

    private Snapshot Load()
    {
        string? name = null;
        var ignore = new List<string>();
        IReadOnlyList<int> thumbnailSizes = DefaultThumbnailSizes;
        var web = DefaultWeb;

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

                if (table.TryGetValue("web", out var w) && w is TomlTable)
                {
                    web = ParseWeb(_paths.ConfigFile);
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "解析 config.toml 失败，使用默认配置");
            }
        }

        return new Snapshot(name, ignore, thumbnailSizes, BuildMatcher(ignore), web);
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

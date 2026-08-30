using System.Security.Cryptography;

namespace Hawk.Server.Core;

/// <summary>
/// 服务启动设置：素材库路径、监听端口、访问 token。
/// 桌面版由 Electron 通过命令行 / 环境变量传入（见 architecture.md）。
/// </summary>
public sealed class ServerSettings
{
    public const int DefaultPort = 27371;

    public required string LibraryRoot { get; init; }
    public int Port { get; init; } = DefaultPort;
    public required string Token { get; init; }

    /// <summary>周期对账扫描间隔（秒），0 关闭。文件监听可能静默丢事件，周期扫描（复用哈希、不读内容）保证最终一致</summary>
    public int RescanIntervalSeconds { get; init; } = 60;

    /// <summary>局域网 web 查看托管的前端静态文件目录（Electron 传入 web/dist）；不存在则不托管</summary>
    public string? WebDist { get; init; }

    public static ServerSettings FromArgs(string[] args)
    {
        string? library = Environment.GetEnvironmentVariable("HAWK_LIBRARY");
        int? port = ParseInt(Environment.GetEnvironmentVariable("HAWK_PORT"));
        var token = Environment.GetEnvironmentVariable("HAWK_TOKEN");
        string? webDist = Environment.GetEnvironmentVariable("HAWK_WEB_DIST");

        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--library" when i + 1 < args.Length:
                    library = args[++i];
                    break;
                case "--port" when i + 1 < args.Length:
                    port = ParseInt(args[++i]);
                    break;
                case "--web-dist" when i + 1 < args.Length:
                    webDist = args[++i];
                    break;
            }
        }

        if (string.IsNullOrWhiteSpace(library))
        {
            Console.Error.WriteLine("用法: hawk-server --library <素材库路径> [--port <端口>]");
            Console.Error.WriteLine("环境变量: HAWK_LIBRARY / HAWK_PORT / HAWK_TOKEN / HAWK_RESCAN_INTERVAL(对账扫描间隔秒,0 关闭,默认 60)");
            Environment.Exit(2);
        }

        if (!Directory.Exists(library))
        {
            Console.Error.WriteLine($"素材库目录不存在: {library}");
            Environment.Exit(2);
        }

        // token 只存在于进程环境中；未传入时（开发场景）生成随机 token 并打印到 stdout
        token ??= Convert.ToHexString(RandomNumberGenerator.GetBytes(32)).ToLowerInvariant();

        return new ServerSettings
        {
            LibraryRoot = Path.GetFullPath(library),
            Port = port ?? DefaultPort,
            Token = token,
            RescanIntervalSeconds = ParseInt(Environment.GetEnvironmentVariable("HAWK_RESCAN_INTERVAL")) ?? 60,
            WebDist = webDist,
        };
    }

    private static int? ParseInt(string? value) => int.TryParse(value, out var n) ? n : null;
}

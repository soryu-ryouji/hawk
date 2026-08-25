using System.Reflection;
using System.Runtime.InteropServices;
using Hawk.Server.Core;

namespace Hawk.Server.Api;

public static class AppEndpoints
{
    public static readonly string Version =
        typeof(AppEndpoints).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()
            ?.InformationalVersion?.Split('+')[0] ?? "1.0.0";

    public sealed record AppInfo(string Version, string Platform, string ExecPath);

    public static void MapAppEndpoints(this IEndpointRouteBuilder app)
    {
        // 就绪探活：无需 token， Electron 壳轮询此端点确认后端就绪
        app.MapGet("/health", () => Results.Ok()).WithTags("app");

        app.MapGet("/api/v1/app/info", () =>
            {
                var platform = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "windows"
                    : RuntimeInformation.IsOSPlatform(OSPlatform.OSX) ? "macos"
                    : "linux";
                var info = new AppInfo(Version, platform, Environment.ProcessPath ?? "");
                return TypedResults.Ok(Envelope<AppInfo>.Ok(info));
            })
            .WithTags("app");
    }
}

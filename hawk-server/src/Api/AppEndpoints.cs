using System.Reflection;
using System.Runtime.InteropServices;
using Hawk.Server.Core;
using Microsoft.AspNetCore.Cors;
using Microsoft.AspNetCore.Http;

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

        // Token 发现：浏览器插件零配置接入（插件端见 hawk-browser-extension）。
        // 安全性依赖两点：响应不带 CORS 头（DisableCors，跨源网页 JS 读不到，扩展持 host_permissions 可读）；
        // Host 限定环回地址（防 DNS rebinding 伪装同源读取）。
        app.MapGet("/api/v1/app/token", (HttpContext ctx, ServerSettings settings) =>
            {
                var host = ctx.Request.Host.Host;
                var loopback = host is "127.0.0.1" or "localhost" or "::1" or "[::1]";
                if (!loopback)
                {
                    return Results.BadRequest(new ErrorEnvelope("error", new ErrorBody("INVALID_HOST", "token discovery requires loopback host")));
                }
                return TypedResults.Ok(Envelope<string>.Ok(settings.Token));
            })
            .WithMetadata(new DisableCorsAttribute())
            .WithTags("app");
    }
}

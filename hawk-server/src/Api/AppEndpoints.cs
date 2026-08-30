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

    public sealed record AppInfo(string Version, string Platform, string ExecPath, string Access);

    /// <summary>启动状态查询：初始索引后台构建期间供客户端轮询进度（见 server-rest-api-v1.md「app」节）</summary>
    public sealed record StartupInfo(string Status, string? Phase, int? Processed, int? Total, string? Message);

    /// <summary>后台任务积压快照:缩略图/调色板 worker 队列</summary>
    public sealed record TaskBacklog(int Pending, int Active);

    /// <summary>索引管道积压:Pending=排队 job+写入防抖路径,Active=扫描中为 1;扫描期间携带阶段进度</summary>
    public sealed record IndexBacklog(int Pending, int Active, string? Phase, int? Processed, int? Total);

    public sealed record TaskStatus(TaskBacklog Thumbnail, IndexBacklog Index);

    public static void MapAppEndpoints(this IEndpointRouteBuilder app)
    {
        // 就绪探活：无需 token。初始索引完成前返回 503（Electron 壳与生态客户端轮询 /api/v1/app/startup 获取进度）
        app.MapGet("/health", (Core.StartupState startup) =>
            startup.IsReady ? Results.Ok("ok") : Results.StatusCode(StatusCodes.Status503ServiceUnavailable)).WithTags("app");

        // 启动状态：ready / starting（带进度）/ error（初始索引失败，message 为原因）
        app.MapGet("/api/v1/app/startup", (Core.StartupState startup) =>
            {
                var info = startup.Error is not null
                    ? new StartupInfo("error", null, null, null, startup.Error)
                    : startup.IsReady
                        ? new StartupInfo("ready", null, null, null, null)
                        : new StartupInfo("starting", startup.Phase, startup.Processed, startup.Total, null);
                return TypedResults.Ok(Envelope<StartupInfo>.Ok(info));
            })
            .WithTags("app");

        // 后台任务积压:轮询型客户端用(SSE 客户端订阅 task.progress 事件,两者同一份快照)
        app.MapGet("/api/v1/app/status", (Core.ThumbnailWorker worker, Core.IndexPipeline pipeline) =>
            {
                var (pending, active) = worker.Backlog;
                var index = pipeline.IndexProgress();
                var status = new TaskStatus(
                    new TaskBacklog(pending, active),
                    new IndexBacklog(index.Pending, index.Active, index.Phase, index.Processed, index.Total));
                return TypedResults.Ok(Envelope<TaskStatus>.Ok(status));
            })
            .WithTags("app");

        app.MapGet("/api/v1/app/info", (HttpContext ctx) =>
            {
                var platform = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "windows"
                    : RuntimeInformation.IsOSPlatform(OSPlatform.OSX) ? "macos"
                    : "linux";
                var access = ctx.Items[TokenAuthMiddleware.AccessItemKey] as string ?? "admin";
                var info = new AppInfo(Version, platform, Environment.ProcessPath ?? "", access);
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

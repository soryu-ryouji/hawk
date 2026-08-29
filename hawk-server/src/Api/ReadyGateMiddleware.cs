namespace Hawk.Server.Api;

/// <summary>
/// 启动网关：初始索引完成前拒绝一切 /api/* 请求（503 NOT_READY），仅放行 /api/v1/app/startup。
/// /health 与 /openapi 不在 /api 前缀下，由各自端点自行处理（health 按就绪状态返回 200/503）。
/// </summary>
public sealed class ReadyGateMiddleware(RequestDelegate next, Core.StartupState startup)
{
    private const string StartupPath = "/api/v1/app/startup";

    public async Task Invoke(HttpContext ctx)
    {
        var path = ctx.Request.Path.Value ?? string.Empty;
        if (!startup.IsReady
            && path.StartsWith("/api/", StringComparison.Ordinal)
            && !string.Equals(path, StartupPath, StringComparison.Ordinal))
        {
            ctx.Response.StatusCode = StatusCodes.Status503ServiceUnavailable;
            await ctx.Response.WriteAsJsonAsync(
                new ErrorEnvelope("error", new ErrorBody(ErrorCodes.NotReady, "initial index is still building")));
            return;
        }

        await next(ctx);
    }
}

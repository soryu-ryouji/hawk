using Hawk.Server.Core;

namespace Hawk.Server.Api;

/// <summary>
/// Token 鉴权：/api/* 请求必须携带 Authorization: Bearer &lt;token&gt;；
/// SSE 端点（/api/v1/events）无法设置请求头，改用查询参数 ?token=。
/// token 由 Electron 启动时生成并经环境变量传入，见 architecture.md。
/// </summary>
public sealed class TokenAuthMiddleware
{
    private readonly RequestDelegate _next;
    private readonly string _token;

    public TokenAuthMiddleware(RequestDelegate next, ServerSettings settings)
    {
        _next = next;
        _token = settings.Token;
    }

    public async Task Invoke(HttpContext context)
    {
        if (!context.Request.Path.StartsWithSegments("/api"))
        {
            await _next(context);
            return;
        }

        if (IsAuthorized(context))
        {
            await _next(context);
            return;
        }

        context.Response.StatusCode = StatusCodes.Status401Unauthorized;
        await context.Response.WriteAsJsonAsync(new ErrorEnvelope("error", new ErrorBody("UNAUTHORIZED", "missing or invalid token")));
    }

    private bool IsAuthorized(HttpContext context)
    {
        var header = context.Request.Headers.Authorization.ToString();
        if (header.StartsWith("Bearer ", StringComparison.Ordinal) &&
            string.Equals(header["Bearer ".Length..], _token, StringComparison.Ordinal))
        {
            return true;
        }

        return context.Request.Path == "/api/v1/events" &&
               string.Equals(context.Request.Query["token"], _token, StringComparison.Ordinal);
    }
}

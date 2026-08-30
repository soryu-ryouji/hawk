using Hawk.Server.Core;

namespace Hawk.Server.Api;

/// <summary>
/// Token 鉴权：/api/* 请求必须携带 Authorization: Bearer &lt;token&gt;；
/// SSE 端点（/api/v1/events）无法设置请求头，改用查询参数 ?token=。
/// 双 token：admin（Electron 启动时生成，全权限）与 viewer（config.toml [web].token，局域网 web 查看，只读）。
/// token 由 Electron 启动时生成并经环境变量传入，见 architecture.md。
/// 例外：GET /api/v1/app/token（token 发现端点）无鉴权，安全性见 AppEndpoints。
/// 鉴权结果写入 HttpContext.Items["hawk.access"]，app/info 据此报告访问级别。
/// </summary>
public sealed class TokenAuthMiddleware
{
    public const string AccessItemKey = "hawk.access";

    private readonly RequestDelegate _next;
    private readonly string _adminToken;
    private readonly LibraryConfig _config;

    public TokenAuthMiddleware(RequestDelegate next, ServerSettings settings, LibraryConfig config)
    {
        _next = next;
        _adminToken = settings.Token;
        _config = config;
    }

    public async Task Invoke(HttpContext context)
    {
        if (!context.Request.Path.StartsWithSegments("/api"))
        {
            await _next(context);
            return;
        }

        // Token 发现端点本身无鉴权（CORS/Host 约束见 AppEndpoints）
        if (HttpMethods.IsGet(context.Request.Method) && context.Request.Path == "/api/v1/app/token")
        {
            await _next(context);
            return;
        }

        var access = ResolveAccess(context);
        if (access is null)
        {
            context.Response.StatusCode = StatusCodes.Status401Unauthorized;
            await context.Response.WriteAsJsonAsync(new ErrorEnvelope("error", new ErrorBody("UNAUTHORIZED", "missing or invalid token")));
            return;
        }

        if (access == "viewer" && !IsViewerAllowed(context.Request))
        {
            context.Response.StatusCode = StatusCodes.Status403Forbidden;
            await context.Response.WriteAsJsonAsync(new ErrorEnvelope("error", new ErrorBody("READ_ONLY", "viewer token is read-only")));
            return;
        }

        context.Items[AccessItemKey] = access;
        await _next(context);
    }

    /// <summary>viewer（局域网 web 查看）仅放行只读端点；写端点一律 403 READ_ONLY</summary>
    private static bool IsViewerAllowed(HttpRequest request)
    {
        if (HttpMethods.IsGet(request.Method))
        {
            return true;
        }

        // 查询类 POST（复杂过滤结构），语义只读
        return request.Path.Value is "/api/v1/item/list" or "/api/v1/item/skeleton";
    }

    /// <summary>返回 access 级别（admin/viewer），token 无效返回 null</summary>
    private string? ResolveAccess(HttpContext context)
    {
        var header = context.Request.Headers.Authorization.ToString();
        if (header.StartsWith("Bearer ", StringComparison.Ordinal))
        {
            var token = header["Bearer ".Length..];
            if (string.Equals(token, _adminToken, StringComparison.Ordinal))
            {
                return "admin";
            }

            if (IsViewerToken(token))
            {
                return "viewer";
            }
        }

        // EventSource 与 <img> 均无法设置请求头，这几个 GET 端点放行查询参数 token
        var allowQueryToken = HttpMethods.IsGet(context.Request.Method) &&
            (context.Request.Path == "/api/v1/events" ||
             context.Request.Path == "/api/v1/item/thumbnail" ||
             context.Request.Path == "/api/v1/item/file");
        if (allowQueryToken)
        {
            var token = context.Request.Query["token"].ToString();
            if (string.Equals(token, _adminToken, StringComparison.Ordinal))
            {
                return "admin";
            }

            if (IsViewerToken(token))
            {
                return "viewer";
            }
        }

        return null;
    }

    private bool IsViewerToken(string token)
    {
        var web = _config.Current.Web;
        return web.Enabled && !string.IsNullOrEmpty(web.Token) &&
               string.Equals(token, web.Token, StringComparison.Ordinal);
    }
}

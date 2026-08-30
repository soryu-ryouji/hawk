using Hawk.Server.Core;

namespace Hawk.Server.Api;

/// <summary>
/// view 端点：视图偏好（排序记忆）。条目以 scope 键扁平存储于 .hawk/view.toml（参与同步）：
/// "folder:\u003c路径\u003e"（"" 为库根，继承由前端沿父链解析）/"category:\u003c名\u003e"/"tag:\u003c名\u003e"。
/// 偏好与索引/元数据无耦合，注册表自带锁，端点直接读写（不经过索引流水线）。
/// </summary>
public static class ViewEndpoints
{
    public sealed record ViewPreferencePutRequest(string Scope, string OrderBy, string Order);

    public static void MapViewEndpoints(this IEndpointRouteBuilder app)
    {
        var view = app.MapGroup("/api/v1/view").WithTags("view");

        view.MapGet("/preferences", (ViewPreferences prefs) =>
            TypedResults.Ok(Envelope<Dictionary<string, ViewSort>>.Ok(new Dictionary<string, ViewSort>(prefs.Snapshot()))));

        view.MapPut("/preference", (ViewPreferencePutRequest req, ViewPreferences prefs) =>
        {
            if (!ViewPreferences.TryParseScope(req.Scope, out var scope))
            {
                throw ApiException.InvalidParam($"非法作用域: {req.Scope}");
            }

            if (!ViewPreferences.TryNormalizeSort(req.OrderBy, req.Order, out var sort))
            {
                throw ApiException.InvalidParam($"非法排序值: {req.OrderBy}/{req.Order}");
            }

            prefs.Set(scope, sort);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });

        view.MapDelete("/preference", (string scope, ViewPreferences prefs) =>
        {
            if (!ViewPreferences.TryParseScope(scope, out var normalized))
            {
                throw ApiException.InvalidParam($"非法作用域: {scope}");
            }

            prefs.Delete(normalized);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });
    }
}

using System.Text.Json;
using Hawk.Server.Core;
using Microsoft.AspNetCore.Http.Json;
using Microsoft.Extensions.Options;

namespace Hawk.Server.Api;

/// <summary>
/// SSE 订阅素材库变更（item.added / item.updated / item.trashed / item.restored / item.removed）。
/// EventSource 无法设置请求头，token 经查询参数传递（鉴权在 TokenAuthMiddleware）。
/// </summary>
public static class EventsEndpoints
{
    public static void MapEventsEndpoints(this IEndpointRouteBuilder app)
    {
        app.MapGet("/api/v1/events", async (HttpContext ctx, EventBus bus, IOptions<JsonOptions> json) =>
        {
            ctx.Response.StatusCode = StatusCodes.Status200OK;
            ctx.Response.ContentType = "text/event-stream";
            ctx.Response.Headers.CacheControl = "no-cache";
            await ctx.Response.StartAsync();

            var reader = bus.Subscribe();
            var options = json.Value.SerializerOptions;
            try
            {
                await foreach (var ev in reader.ReadAllAsync(ctx.RequestAborted))
                {
                    var data = JsonSerializer.Serialize(ev.Payload, options);
                    await ctx.Response.WriteAsync($"event: {ev.Type}\ndata: {data}\n\n", ctx.RequestAborted);
                    await ctx.Response.Body.FlushAsync(ctx.RequestAborted);
                }
            }
            catch (OperationCanceledException)
            {
                // 客户端断开
            }
            finally
            {
                bus.Unsubscribe(reader);
            }
        }).WithTags("events").ExcludeFromDescription(); // SSE 流式响应不纳入 OpenAPI schema
    }
}

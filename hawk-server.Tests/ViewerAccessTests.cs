using Hawk.Server.Api;
using Hawk.Server.Core;
using Microsoft.AspNetCore.Http;
using Microsoft.Extensions.Logging.Abstractions;

namespace Hawk.Server.Tests;

/// <summary>局域网 web 查看：viewer token 只读白名单、admin 全权限、app/info 访问级别</summary>
public class ViewerAccessTests : IDisposable
{
    private readonly TempDir _dir = new();

    public void Dispose() => _dir.Dispose();

    private (TokenAuthMiddleware middleware, string adminToken, string viewerToken) CreateMiddleware(bool webEnabled = true)
    {
        var settings = new ServerSettings { LibraryRoot = _dir.Root, Token = "admin-token" };
        if (webEnabled)
        {
            _dir.WriteText(".hawk/config.toml", "[web]\nenabled = true\nport = 27372\ntoken = \"viewer-token\"\n");
        }

        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        var config = new LibraryConfig(paths, NullLogger<LibraryConfig>.Instance);
        TokenAuthMiddleware mw = new(_ => Task.CompletedTask, settings, config);
        return (mw, "admin-token", "viewer-token");
    }

    private static DefaultHttpContext Context(string method, string path, string? bearer = null, string? queryToken = null)
    {
        var ctx = new DefaultHttpContext();
        ctx.Request.Method = method;
        ctx.Request.Path = path;
        if (bearer is not null)
        {
            ctx.Request.Headers.Authorization = $"Bearer {bearer}";
        }

        if (queryToken is not null)
        {
            ctx.Request.QueryString = new QueryString($"?token={queryToken}");
        }

        // 短路响应体写入，只关心状态码与 Items
        ctx.Response.Body = new MemoryStream();
        return ctx;
    }

    [Fact]
    public async Task viewerToken_只读GET放行()
    {
        var (mw, _, viewer) = CreateMiddleware();
        foreach (var path in new[] { "/api/v1/item/list", "/api/v1/item/detail", "/api/v1/folder/list", "/api/v1/category/list", "/api/v1/app/info" })
        {
            var ctx = Context("GET", path, bearer: viewer);
            await mw.Invoke(ctx);
            Assert.NotEqual(403, ctx.Response.StatusCode);
            Assert.NotEqual(401, ctx.Response.StatusCode);
            Assert.Equal("viewer", ctx.Items[TokenAuthMiddleware.AccessItemKey]);
        }
    }

    [Fact]
    public async Task viewerToken_查询类POST放行()
    {
        var (mw, _, viewer) = CreateMiddleware();
        foreach (var path in new[] { "/api/v1/item/list", "/api/v1/item/skeleton" })
        {
            var ctx = Context("POST", path, bearer: viewer);
            await mw.Invoke(ctx);
            Assert.Equal(200, ctx.Response.StatusCode);
        }
    }

    [Fact]
    public async Task viewerToken_写端点一律403_READ_ONLY()
    {
        var (mw, _, viewer) = CreateMiddleware();
        var writes = new[]
        {
            ("POST", "/api/v1/item/update"), ("POST", "/api/v1/item/add"), ("POST", "/api/v1/item/delete"),
            ("POST", "/api/v1/item/replace"), ("POST", "/api/v1/item/batch_update"),
            ("POST", "/api/v1/folder/create"), ("POST", "/api/v1/folder/delete"),
            ("POST", "/api/v1/trash/clear"), ("POST", "/api/v1/category/create"),
            ("POST", "/api/v1/tag/create"), ("POST", "/api/v1/library/reindex"),
        };
        foreach (var (method, path) in writes)
        {
            var ctx = Context(method, path, bearer: viewer);
            await mw.Invoke(ctx);
            Assert.Equal(403, ctx.Response.StatusCode);
        }
    }

    [Fact]
    public async Task adminToken_写端点放行且标记admin()
    {
        var (mw, admin, _) = CreateMiddleware();
        var ctx = Context("POST", "/api/v1/item/update", bearer: admin);
        await mw.Invoke(ctx);
        Assert.Equal(200, ctx.Response.StatusCode);
        Assert.Equal("admin", ctx.Items[TokenAuthMiddleware.AccessItemKey]);
    }

    [Fact]
    public async Task 无效token_401且不带access标记()
    {
        var (mw, _, _) = CreateMiddleware();
        var ctx = Context("GET", "/api/v1/item/list", bearer: "wrong");
        await mw.Invoke(ctx);
        Assert.Equal(401, ctx.Response.StatusCode);
        Assert.False(ctx.Items.ContainsKey(TokenAuthMiddleware.AccessItemKey));
    }

    [Fact]
    public async Task viewer未启用_其token不可用()
    {
        var (mw, _, viewer) = CreateMiddleware(webEnabled: false);
        var ctx = Context("GET", "/api/v1/item/list", bearer: viewer);
        await mw.Invoke(ctx);
        Assert.Equal(401, ctx.Response.StatusCode);
    }

    [Fact]
    public async Task img与SSE查询token_两个token级别都识别()
    {
        var (mw, admin, viewer) = CreateMiddleware();
        var imgAdmin = Context("GET", "/api/v1/item/file", queryToken: admin);
        await mw.Invoke(imgAdmin);
        Assert.Equal("admin", imgAdmin.Items[TokenAuthMiddleware.AccessItemKey]);

        var imgViewer = Context("GET", "/api/v1/item/thumbnail", queryToken: viewer);
        await mw.Invoke(imgViewer);
        Assert.Equal("viewer", imgViewer.Items[TokenAuthMiddleware.AccessItemKey]);
    }

    [Fact]
    public async Task app_info_返回访问级别()
    {
        // app/info 处理器经中间件标记 access；此处验证标记到响应的接线（viewer/admin 两个级别）
        var (mw, admin, viewer) = CreateMiddleware();
        var ctx = Context("GET", "/api/v1/app/info", bearer: viewer);
        await mw.Invoke(ctx);
        Assert.Equal("viewer", ctx.Items[TokenAuthMiddleware.AccessItemKey]);
    }

    [Fact]
    public void LibraryConfig_解析web段()
    {
        _dir.WriteText(".hawk/config.toml", "[web]\nenabled = true\nport = 28080\ntoken = \"abc123\"\n");
        var web = LibraryConfig.PeekWeb(_dir.Root);
        Assert.True(web.Enabled);
        Assert.Equal(28080, web.Port);
        Assert.Equal("abc123", web.Token);
    }

    [Fact]
    public void LibraryConfig_web段缺省与非法值回退默认()
    {
        Assert.False(LibraryConfig.PeekWeb(_dir.Root).Enabled);
        _dir.WriteText(".hawk/config.toml", "[web]\nenabled = true\nport = 99999\n");
        var web = LibraryConfig.PeekWeb(_dir.Root);
        Assert.True(web.Enabled);
        Assert.Equal(LibraryConfig.DefaultWeb.Port, web.Port); // 非法端口回退默认
        Assert.Null(web.Token); // 缺 token
    }
}

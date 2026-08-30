using System.Net;
using System.Net.Sockets;
using System.Text.Json;
using Hawk.Server.Api;
using Hawk.Server.Core;
using Microsoft.AspNetCore.Hosting.Server;
using Microsoft.AspNetCore.Hosting.Server.Features;
using Serilog;

var settings = ServerSettings.FromArgs(args);
var port = ResolvePort(settings.Port);

var builder = WebApplication.CreateBuilder(args);
builder.WebHost.UseUrls(BuildUrls(settings, port));
builder.Host.UseSerilog((_, lc) => lc.WriteTo.Console());

builder.Services.ConfigureHttpJsonOptions(options =>
{
    // API 契约统一 snake_case；null 字段省略（如 url、annotation 为空时）
    options.SerializerOptions.PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower;
    options.SerializerOptions.DefaultIgnoreCondition = System.Text.Json.Serialization.JsonIgnoreCondition.WhenWritingNull;
});
builder.Services.AddOpenApi();
builder.Services.AddCors();

builder.Services.AddSingleton(settings);
builder.Services.AddSingleton(sp =>
{
    var paths = new LibraryPaths(settings.LibraryRoot);
    paths.EnsureLayout();
    return paths;
});
builder.Services.AddSingleton<LibraryConfig>();
builder.Services.AddSingleton<MetadataStore>();
builder.Services.AddSingleton<CategoryRegistry>();
builder.Services.AddSingleton<TagRegistry>();
builder.Services.AddSingleton<ItemIndex>();
builder.Services.AddSingleton<ThumbnailService>();
builder.Services.AddSingleton<ColorService>();
builder.Services.AddSingleton<ThumbnailWorker>();
builder.Services.AddSingleton<EventBus>();
builder.Services.AddSingleton<LibraryScanner>();
builder.Services.AddSingleton<TaxonomyMigrator>();
builder.Services.AddSingleton<IndexPipeline>();
builder.Services.AddSingleton<LibraryWatcher>();
builder.Services.AddSingleton<StartupState>();

var app = builder.Build();

app.UseCors(policy => policy.AllowAnyOrigin().AllowAnyMethod().AllowAnyHeader());

// 局域网 web 查看：Electron 传入 web/dist 目录时托管前端静态文件（页面无鉴权，API 仍全鉴权）。
// SPA 回退到 index.html，/api 与 /health 不受影响。目录不存在（如未构建）则静默跳过，仅提供 API。
if (!string.IsNullOrEmpty(settings.WebDist) && Directory.Exists(settings.WebDist))
{
    var fileProvider = new Microsoft.Extensions.FileProviders.PhysicalFileProvider(settings.WebDist);
    // 缓存策略：vite 产物带内容哈希，/assets/ 下 immutable 长缓存；index.html 与其余路径 no-cache
    // （每次校验、304 廉价）——不设缓存头时浏览器启发式缓存旧 HTML，重建后手机端仍跑旧 bundle
    // （2026-08 移动端"横排溢出未解决"实为手机拿的是构建前旧版本）
    void SetCacheHeaders(Microsoft.AspNetCore.StaticFiles.StaticFileResponseContext ctx)
    {
        var path = ctx.Context.Request.Path.Value ?? string.Empty;
        ctx.Context.Response.Headers["Cache-Control"] = path.StartsWith("/assets/", StringComparison.Ordinal)
            ? "public, max-age=31536000, immutable"
            : "no-cache";
    }

    app.UseDefaultFiles(new DefaultFilesOptions { FileProvider = fileProvider });
    app.UseStaticFiles(new StaticFileOptions { FileProvider = fileProvider, OnPrepareResponse = SetCacheHeaders });
    app.MapFallbackToFile("index.html", new StaticFileOptions { FileProvider = fileProvider, OnPrepareResponse = SetCacheHeaders });
}

app.UseMiddleware<ErrorHandlingMiddleware>();
app.UseMiddleware<TokenAuthMiddleware>();
app.UseMiddleware<ReadyGateMiddleware>();

app.MapOpenApi(); // /openapi/v1.json，schema 即契约，无需 token
app.MapAppEndpoints();
app.MapLibraryEndpoints();
app.MapFolderEndpoints();
app.MapItemEndpoints();
app.MapTrashEndpoints();
app.MapTaxonomyEndpoints();
app.MapEventsEndpoints();

// 启动顺序（server-csharp.md）：先监听端口（先监听、扫描后台进行），初始索引完成后才算就绪。
// 客户端（Electron 壳/生态接入）轮询 GET /api/v1/app/startup 获取进度与就绪状态，初始索引期间除该端点外一律 503 NOT_READY。
var pipeline = app.Services.GetRequiredService<IndexPipeline>();
var watcher = app.Services.GetRequiredService<LibraryWatcher>();
var startup = app.Services.GetRequiredService<StartupState>();

pipeline.OnScanProgress = startup.Report;
pipeline.AttachThumbnailWorker(app.Services.GetRequiredService<ThumbnailWorker>());

pipeline.Start();
watcher.FileUpsert += pipeline.NotifyUpsert;
watcher.Deleted += pipeline.NotifyDeleted;
watcher.Moved += pipeline.NotifyMoved;
watcher.FolderCreated += _ => pipeline.NotifyFolderChanged(FolderChangedPayload.ReasonExternal);
watcher.ConfigChanged += pipeline.NotifyConfigChanged;
watcher.RegistryChanged += pipeline.NotifyRegistryChanged;
watcher.Overflowed += pipeline.NotifyOverflow;
watcher.Start(); // 事件先入队缓冲，与初始扫描天然去重

var initialScan = pipeline.RunScanAsync(full: false); // 后台构建，不阻塞端口监听

app.Lifetime.ApplicationStopping.Register(() =>
{
    watcher.Dispose();
    pipeline.Dispose();
});

await app.StartAsync();

var address = app.Services.GetRequiredService<IServer>().Features
    .Get<IServerAddressesFeature>()!.Addresses.First();
app.Logger.LogInformation("hawk-server 监听 {Address}，素材库: {Library}，初始索引后台构建中", address, settings.LibraryRoot);

// 初始索引结果异步落定：就绪后 /health 转 200、API 网关放行；失败则记入启动状态供客户端查询
_ = Task.Run(async () =>
{
    try
    {
        await initialScan;
        startup.MarkReady();
        app.Logger.LogInformation("初始索引完成，hawk-server 就绪");
    }
    catch (Exception ex)
    {
        startup.Fail(ex);
        app.Logger.LogError(ex, "初始索引构建失败");
    }
});

await app.WaitForShutdownAsync();

/// <summary>默认端口被占用时回退为动态分配（返回 0 由 Kestrel 选端口）</summary>
static int ResolvePort(int preferred)
{
    try
    {
        var listener = new TcpListener(IPAddress.Loopback, preferred);
        listener.Start();
        listener.Stop();
        return preferred;
    }
    catch (SocketException)
    {
        return 0;
    }
}

/// <summary>
/// 监听地址：桌面 API 恒为环回；[web] 启用且配好 token 时追加局域网绑定。
/// LAN 端口被占用直接启动失败（报错可见），不做静默回退——局域网访问依赖固定端口。
/// </summary>
static string BuildUrls(ServerSettings settings, int port)
{
    var urls = $"http://127.0.0.1:{port}";
    var web = LibraryConfig.PeekWeb(settings.LibraryRoot);
    if (web.Enabled)
    {
        if (string.IsNullOrEmpty(web.Token))
        {
            Console.Error.WriteLine("[web] enabled 但缺少 token，局域网查看未启动（在设置面板配置 token）");
            return urls;
        }

        try
        {
            var probe = new TcpListener(IPAddress.Any, web.Port);
            probe.Start();
            probe.Stop();
            urls += $";http://0.0.0.0:{web.Port}";
        }
        catch (SocketException)
        {
            Console.Error.WriteLine($"局域网查看端口 {web.Port} 被占用，hawk-server 启动失败：请更换端口或关闭占用进程");
            Environment.Exit(3);
        }
    }

    return urls;
}

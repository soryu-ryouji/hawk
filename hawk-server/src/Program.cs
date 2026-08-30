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
builder.Services.AddSingleton<IndexDb>();
builder.Services.AddSingleton<MetadataStore>();
builder.Services.AddSingleton<CategoryRegistry>();
builder.Services.AddSingleton<TagRegistry>();
builder.Services.AddSingleton<ViewPreferences>();
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
app.MapViewEndpoints();
app.MapEventsEndpoints();

// 启动顺序（server-csharp.md）：Kestrel 先监听（注水/缓存重建期间 startup 端点即可答 starting，
// 客户端有进度反馈），随后装配索引流水线——内存索引由元数据缓存（SQLite 快路径/TOML 回退）注水，
// 就绪不再等待全库扫描；停机期间的文件增删改由文件监听实时事件 + 后台对账扫描 + 周期对账收敛。
// 客户端（Electron 壳/生态接入）轮询 GET /api/v1/app/startup；初始注水期间除该端点外一律 503 NOT_READY。
var startup = app.Services.GetRequiredService<StartupState>();

app.Lifetime.ApplicationStopping.Register(() =>
{
    app.Services.GetRequiredService<LibraryWatcher>().Dispose();
    app.Services.GetRequiredService<IndexPipeline>().Dispose();
});

await app.StartAsync();

// Kestrel 已监听：以下单例的首次构造（元数据副本注水/缓存重建）期间，startup 端点持续可答
var pipeline = app.Services.GetRequiredService<IndexPipeline>();
var watcher = app.Services.GetRequiredService<LibraryWatcher>();
var prefs = app.Services.GetRequiredService<ViewPreferences>();

pipeline.OnScanProgress = startup.Report;
pipeline.AttachThumbnailWorker(app.Services.GetRequiredService<ThumbnailWorker>());

pipeline.Start();
watcher.FileUpsert += pipeline.NotifyUpsert;
watcher.Deleted += pipeline.NotifyDeleted;
watcher.Moved += pipeline.NotifyMoved;
watcher.FolderCreated += _ => pipeline.NotifyFolderChanged(FolderChangedPayload.ReasonExternal);
watcher.ConfigChanged += pipeline.NotifyConfigChanged;
watcher.RegistryChanged += pipeline.NotifyRegistryChanged;
watcher.PreferencesChanged += prefs.Reload; // 视图偏好与索引无耦合,直接重载(网盘同步落地同理)
watcher.Overflowed += pipeline.NotifyOverflow;
watcher.Start(); // 事件先入队缓冲，与对账扫描天然去重

startup.MarkReady();
app.Logger.LogInformation("hawk-server 已就绪（内存索引已由缓存注水），后台对账扫描进行中");

// 全库对账扫描转后台：完成前停机期间的删除/新增短暂残留（watcher 实时事件已覆盖运行期变更），
// 失败不置启动错误——周期对账（默认 60s）兜底重试
_ = Task.Run(async () =>
{
    try
    {
        await pipeline.RunScanAsync(full: false);
    }
    catch (Exception ex)
    {
        app.Logger.LogError(ex, "后台对账扫描失败（周期对账将重试）");
    }
});

var address = app.Services.GetRequiredService<IServer>().Features
    .Get<IServerAddressesFeature>()!.Addresses.First();
app.Logger.LogInformation("hawk-server 监听 {Address}，素材库: {Library}", address, settings.LibraryRoot);

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

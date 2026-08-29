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
builder.WebHost.UseUrls($"http://127.0.0.1:{port}");
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
builder.Services.AddSingleton<EventBus>();
builder.Services.AddSingleton<LibraryScanner>();
builder.Services.AddSingleton<IndexPipeline>();
builder.Services.AddSingleton<LibraryWatcher>();
builder.Services.AddSingleton<StartupState>();

var app = builder.Build();

app.UseCors(policy => policy.AllowAnyOrigin().AllowAnyMethod().AllowAnyHeader());
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

pipeline.Start();
watcher.FileUpsert += pipeline.NotifyUpsert;
watcher.Deleted += pipeline.NotifyDeleted;
watcher.Moved += pipeline.NotifyMoved;
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

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
builder.Services.AddSingleton<EventBus>();
builder.Services.AddSingleton<LibraryScanner>();
builder.Services.AddSingleton<IndexPipeline>();
builder.Services.AddSingleton<LibraryWatcher>();

var app = builder.Build();

app.UseCors(policy => policy.AllowAnyOrigin().AllowAnyMethod().AllowAnyHeader());
app.UseMiddleware<ErrorHandlingMiddleware>();
app.UseMiddleware<TokenAuthMiddleware>();

app.MapOpenApi(); // /openapi/v1.json，schema 即契约，无需 token
app.MapAppEndpoints();
app.MapLibraryEndpoints();
app.MapFolderEndpoints();
app.MapItemEndpoints();
app.MapTrashEndpoints();
app.MapTaxonomyEndpoints();
app.MapEventsEndpoints();

// 启动顺序（server-csharp.md）：先开文件监听（事件入缓冲队列），再扫描建索引，就绪后才开放端口
var pipeline = app.Services.GetRequiredService<IndexPipeline>();
var watcher = app.Services.GetRequiredService<LibraryWatcher>();

pipeline.Start();
watcher.FileUpsert += pipeline.NotifyUpsert;
watcher.Deleted += pipeline.NotifyDeleted;
watcher.Moved += pipeline.NotifyMoved;
watcher.ConfigChanged += pipeline.NotifyConfigChanged;
watcher.RegistryChanged += pipeline.NotifyRegistryChanged;
watcher.Overflowed += pipeline.NotifyOverflow;
watcher.Start();

await pipeline.RunScanAsync(full: false); // 阻塞至初始索引完成

app.Lifetime.ApplicationStopping.Register(() =>
{
    watcher.Dispose();
    pipeline.Dispose();
});

await app.StartAsync();

var address = app.Services.GetRequiredService<IServer>().Features
    .Get<IServerAddressesFeature>()!.Addresses.First();
app.Logger.LogInformation("hawk-server 就绪: {Address}，素材库: {Library}", address, settings.LibraryRoot);

// Electron 主进程解析此行获取端口与 token
Console.WriteLine($"HAWK_READY {address} token={settings.Token}");

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

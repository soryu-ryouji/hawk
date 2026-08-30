using Hawk.Server.Core;

namespace Hawk.Server.Api;

public static class LibraryEndpoints
{
    /// <param name="ThumbnailSizes">缩略图尺寸白名单（config.toml thumbnail_sizes），前端据此构建网格 srcset</param>
    public sealed record LibraryInfo(string Name, string Path, long ModificationTime, string ApplicationVersion, int[] ThumbnailSizes);

    public static void MapLibraryEndpoints(this IEndpointRouteBuilder app)
    {
        var group = app.MapGroup("/api/v1/library").WithTags("library");

        group.MapGet("/info", (LibraryPaths paths, LibraryConfig config) =>
            {
                var root = new DirectoryInfo(paths.Root);
                var info = new LibraryInfo(
                    config.Current.Name ?? root.Name,
                    paths.Root,
                    LibraryPaths.ToUnixMs(root.LastWriteTimeUtc),
                    AppEndpoints.Version,
                    config.Current.ThumbnailSizes.ToArray());
                return TypedResults.Ok(Envelope<LibraryInfo>.Ok(info));
            });

        // 全量重建索引：重算全部哈希，异步执行，立即返回
        group.MapPost("/reindex", (IndexPipeline pipeline) =>
        {
            pipeline.RequestScan(full: true);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });

        // 刷新缓存：忽略快照强制遍历全部文件做复用判定（不读文件内容），收敛监听漏事件与直接改目录。异步执行，立即返回
        group.MapPost("/rescan", (IndexPipeline pipeline) =>
        {
            pipeline.RequestRescan();
            return TypedResults.Ok(new Envelope<object>("success", null));
        });
    }
}

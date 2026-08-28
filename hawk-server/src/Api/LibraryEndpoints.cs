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

        // 全量重建索引：异步执行，立即返回
        group.MapPost("/reindex", (IndexPipeline pipeline) =>
        {
            pipeline.RequestScan(full: true);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });
    }
}

using Hawk.Server.Core;

namespace Hawk.Server.Api;

public static class TrashEndpoints
{
    public static void MapTrashEndpoints(this IEndpointRouteBuilder app)
    {
        var group = app.MapGroup("/api/v1/trash").WithTags("trash");

        // 清空回收站：先由流水线清理索引位置、元数据与缓存（缩略图/调色板），再物理删除。
        // 顺序不能颠倒：若先物理删除，watcher 的 Deleted 事件可能先于 ClearTrashJob 把位置从索引摘除，
        // 导致 DoClearTrash 找不到位置、元数据与缓存泄漏（Windows 上 watcher 延迟低，必现）。
        group.MapPost("/clear", async (LibraryPaths paths, IndexPipeline pipeline) =>
        {
            await pipeline.SubmitClearTrashAsync();

            foreach (var entry in Directory.EnumerateFileSystemEntries(paths.TrashDir))
            {
                if (Directory.Exists(entry))
                {
                    Directory.Delete(entry, recursive: true);
                }
                else
                {
                    File.Delete(entry);
                }
            }

            return TypedResults.Ok(new Envelope<object>("success", null));
        });
    }
}

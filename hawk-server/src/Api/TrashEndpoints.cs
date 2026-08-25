using Hawk.Server.Core;

namespace Hawk.Server.Api;

public static class TrashEndpoints
{
    public static void MapTrashEndpoints(this IEndpointRouteBuilder app)
    {
        var group = app.MapGroup("/api/v1/trash").WithTags("trash");

        // 清空回收站：物理删除全部内容，元数据与缩略图由流水线清理（不可恢复）
        group.MapPost("/clear", async (LibraryPaths paths, IndexPipeline pipeline) =>
        {
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

            await pipeline.SubmitClearTrashAsync();
            return TypedResults.Ok(new Envelope<object>("success", null));
        });
    }
}

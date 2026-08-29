using Hawk.Server.Core;

namespace Hawk.Server.Api;

/// <summary>
/// 文件夹即素材库中的真实目录。操作直接作用于文件系统，索引由文件监听/流水线同步。
/// </summary>
public static class FolderEndpoints
{
    public sealed record FolderNode(string Path, string Name, FolderNode[] Children, long ModificationTime, int Count);

    public sealed record FolderCreateRequest(string Name, string? ParentPath);
    public sealed record FolderUpdateRequest(string Path, string? Name, string? ParentPath);
    public sealed record FolderPathRequest(string Path);

    public static void MapFolderEndpoints(this IEndpointRouteBuilder app)
    {
        var group = app.MapGroup("/api/v1/folder").WithTags("folder");

        group.MapGet("/list", (LibraryPaths paths, LibraryConfig config, ItemIndex index) =>
            TypedResults.Ok(Envelope<FolderNode>.Ok(BuildTree(paths, config, index))));

        group.MapPost("/create", async (FolderCreateRequest req, LibraryPaths paths, LibraryConfig config, ItemIndex index, IndexPipeline pipeline) =>
        {
            if (!LibraryFs.IsValidName(req.Name))
            {
                throw ApiException.InvalidParam($"非法文件夹名称: {req.Name}");
            }

            var parentRel = req.ParentPath ?? "";
            var parentAbs = ResolveExistingDir(paths, parentRel);
            var targetAbs = Path.Combine(parentAbs, req.Name);
            if (Directory.Exists(targetAbs))
            {
                throw ApiException.FileExists(JoinRel(parentRel, req.Name));
            }

            Directory.CreateDirectory(targetAbs);
            // 目录结构变化广播(folder.changed):本端操作 + 其他客户端的 SSE 刷新统一走事件
            pipeline.NotifyFolderChanged(FolderChangedPayload.ReasonExternal);
            return TypedResults.Ok(Envelope<FolderNode>.Ok(ToNode(paths, config, index, targetAbs)));
        });

        group.MapPost("/update", async (FolderUpdateRequest req, LibraryPaths paths, LibraryConfig config, ItemIndex index, IndexPipeline pipeline) =>
        {
            if (!LibraryPaths.IsValidLibraryPath(req.Path))
            {
                throw ApiException.InvalidParam($"非法文件夹路径: {req.Path}");
            }

            var dirAbs = paths.ToAbsolute(req.Path)!;
            if (!Directory.Exists(dirAbs))
            {
                throw ApiException.FolderNotFound(req.Path);
            }

            var newName = req.Name ?? Path.GetFileName(dirAbs);
            if (!LibraryFs.IsValidName(newName))
            {
                throw ApiException.InvalidParam($"非法文件夹名称: {req.Name}");
            }

            var newParentRel = req.ParentPath ?? LibraryPaths.DirOf(req.Path);
            var newParentAbs = ResolveExistingDir(paths, newParentRel);
            var targetRel = JoinRel(newParentRel, newName);
            if (targetRel == req.Path)
            {
                return TypedResults.Ok(Envelope<FolderNode>.Ok(ToNode(paths, config, index, dirAbs)));
            }

            // 不允许移动到自身子目录
            if (targetRel.StartsWith(req.Path + "/", StringComparison.Ordinal))
            {
                throw ApiException.InvalidParam("不能移动到自身子目录");
            }

            var targetAbs = Path.Combine(newParentAbs, newName);
            if (Directory.Exists(targetAbs) || File.Exists(targetAbs))
            {
                throw ApiException.FileExists(targetRel);
            }

            Directory.Move(dirAbs, targetAbs);
            await pipeline.SubmitDirMoveAsync(dirAbs, targetAbs);
            // DirMoveJob 内会广播 folder.changed,此处不重复通知
            return TypedResults.Ok(Envelope<FolderNode>.Ok(ToNode(paths, config, index, targetAbs)));
        });

        // 删除:整体移入 .hawk/trash/(保留目录结构)
        group.MapPost("/delete", async (FolderPathRequest req, LibraryPaths paths, IndexPipeline pipeline) =>
        {
            if (!LibraryPaths.IsValidLibraryPath(req.Path))
            {
                throw ApiException.InvalidParam($"非法文件夹路径: {req.Path}");
            }

            var dirAbs = paths.ToAbsolute(req.Path)!;
            if (!Directory.Exists(dirAbs))
            {
                throw ApiException.FolderNotFound(req.Path);
            }

            var trashAbs = LibraryFs.FindFreeTrashPath(paths, req.Path, isDirectory: true);
            LibraryFs.EnsureParentDir(trashAbs);
            Directory.Move(dirAbs, trashAbs);
            await pipeline.SubmitDirMoveAsync(dirAbs, trashAbs);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });

        // 恢复：按原路径放回，被占用时报 FILE_EXISTS
        group.MapPost("/restore", async (FolderPathRequest req, LibraryPaths paths, IndexPipeline pipeline) =>
        {
            if (!LibraryPaths.IsValidLibraryPath(req.Path))
            {
                throw ApiException.InvalidParam($"非法文件夹路径: {req.Path}");
            }

            var trashAbs = Path.Combine(new[] { paths.TrashDir }.Concat(req.Path.Split('/')).ToArray());
            if (!Directory.Exists(trashAbs))
            {
                throw ApiException.FolderNotFound(req.Path);
            }

            var targetAbs = paths.ToAbsolute(req.Path)!;
            if (Directory.Exists(targetAbs) || File.Exists(targetAbs))
            {
                throw ApiException.FileExists(req.Path);
            }

            LibraryFs.EnsureParentDir(targetAbs);
            Directory.Move(trashAbs, targetAbs);
            await pipeline.SubmitDirMoveAsync(trashAbs, targetAbs);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });
    }

    /// <summary>实时从文件系统构建文件夹树（排除 .hawk 与被 ignore 的目录），附库内 item 计数</summary>
    private static FolderNode BuildTree(LibraryPaths paths, LibraryConfig config, ItemIndex index) =>
        ToNode(paths, config, index, paths.Root);

    private static FolderNode ToNode(LibraryPaths paths, LibraryConfig config, ItemIndex index, string absDir)
    {
        var counts = index.FolderCounts();
        var info = new DirectoryInfo(absDir);
        var rel = paths.ToRelative(absDir) ?? "";
        var isRoot = rel == "";

        var children = info.EnumerateDirectories()
            .Where(d =>
            {
                var childRel = isRoot ? d.Name : rel + "/" + d.Name;
                if (isRoot && d.Name == LibraryPaths.HawkDirName)
                {
                    return false;
                }

                return !config.IsIgnored(childRel);
            })
            .OrderBy(d => d.Name, StringComparer.OrdinalIgnoreCase)
            .Select(d => ToNode(paths, config, index, d.FullName))
            .ToArray();

        return new FolderNode(rel, info.Name, children, LibraryPaths.ToUnixMs(info.LastWriteTimeUtc), counts.GetValueOrDefault(rel));
    }

    /// <summary>解析父目录：缺省为库根目录；必须已存在</summary>
    private static string ResolveExistingDir(LibraryPaths paths, string rel)
    {
        if (rel == "")
        {
            return paths.Root;
        }

        if (!LibraryPaths.IsValidLibraryPath(rel))
        {
            throw ApiException.InvalidParam($"非法文件夹路径: {rel}");
        }

        var abs = paths.ToAbsolute(rel)!;
        if (!Directory.Exists(abs))
        {
            throw ApiException.FolderNotFound(rel);
        }

        return abs;
    }

    private static string JoinRel(string parentRel, string name) => parentRel == "" ? name : parentRel + "/" + name;
}

using System.Net;
using Microsoft.AspNetCore.StaticFiles;
using Hawk.Server.Core;

namespace Hawk.Server.Api;

public static class ItemEndpoints
{
    public sealed record ItemListRequest
    {
        public string[]? Ids { get; init; }
        public string[]? Keywords { get; init; }
        public string[]? Tags { get; init; }
        public int? Star { get; init; }
        public string[]? Folders { get; init; }
        public string[]? Categories { get; init; }
        public string? CategoriesMatch { get; init; }
        public string[]? ExcludeCategories { get; init; }
        public string[]? ExcludeTags { get; init; }
        public string? Ext { get; init; }
        public string? Annotation { get; init; }
        public string? Url { get; init; }
        public bool InTrash { get; init; }
        public string? OrderBy { get; init; }
        public string? Order { get; init; }
        public int Offset { get; init; }
        public int Limit { get; init; } = 50;
    }

    public sealed record ItemListResponse(ItemDto[] Items, int Total, int Offset, int Limit);

    public sealed record ItemAddRequest
    {
        public string? Path { get; init; }
        public string? Url { get; init; }
        public string? ImgBase64 { get; init; }
        public string? Name { get; init; }
        public string? FolderPath { get; init; }
        public string[]? Tags { get; init; }
        public string? Annotation { get; init; }
    }

    public sealed record ItemAddResponse(ItemDto Item, bool AlreadyExisted);

    public sealed record ItemUpdateRequest
    {
        public required string Id { get; init; }
        public string? Path { get; init; }
        public string? Name { get; init; }
        public string[]? Tags { get; init; }
        public string? FolderPath { get; init; }
        public int? Star { get; init; }
        public string[]? Categories { get; init; }
        public string? Annotation { get; init; }
        public string? Url { get; init; }
    }

    public sealed record ItemIdRequest(string Id, string? Path);
    public sealed record ItemRefreshThumbnailRequest(string Id);

    private static readonly HttpClient Http = new() { Timeout = TimeSpan.FromSeconds(30) };

    public static void MapItemEndpoints(this IEndpointRouteBuilder app)
    {
        var group = app.MapGroup("/api/v1/item").WithTags("item");

        group.MapPost("/list", (ItemListRequest? req, ItemIndex index) =>
        {
            req ??= new ItemListRequest();
            var query = new ItemQuery
            {
                Ids = req.Ids, Keywords = req.Keywords, Tags = req.Tags, Star = req.Star,
                Folders = req.Folders, Categories = req.Categories, CategoriesMatch = req.CategoriesMatch,
                ExcludeCategories = req.ExcludeCategories, ExcludeTags = req.ExcludeTags,
                Ext = req.Ext, Annotation = req.Annotation, Url = req.Url,
                InTrash = req.InTrash, OrderBy = req.OrderBy, Order = req.Order,
                Offset = req.Offset, Limit = req.Limit,
            };
            var items = index.Query(query, out var total);
            var response = new ItemListResponse(items.ToArray(), total, query.Offset, query.Limit);
            return TypedResults.Ok(Envelope<ItemListResponse>.Ok(response));
        });

        group.MapGet("/detail", (string id, ItemIndex index) =>
        {
            var item = index.Get(id) ?? throw ApiException.ItemNotFound(id);
            return TypedResults.Ok(Envelope<ItemDto>.Ok(item.ToDto(trashView: !item.HasLibraryLocations)));
        });

        group.MapGet("/count", (ItemIndex index) => TypedResults.Ok(Envelope<int>.Ok(index.Count())));

        group.MapPost("/add", AddAsync);
        group.MapPost("/update", UpdateAsync);
        group.MapPost("/delete", DeleteAsync);
        group.MapPost("/restore", RestoreAsync);

        group.MapGet("/thumbnail", (string id, int? size, HttpContext ctx, ItemIndex index, ThumbnailService thumbnails, LibraryConfig config) =>
        {
            var actualSize = size ?? 256;
            if (!config.Current.ThumbnailSizes.Contains(actualSize))
            {
                throw ApiException.InvalidParam($"不支持的缩略图尺寸: {actualSize}");
            }

            _ = index.Get(id) ?? throw ApiException.ItemNotFound(id);
            var file = thumbnails.GetPath(id, actualSize);
            if (!File.Exists(file))
            {
                // 首次索引完成前缩略图可能尚未生成
                throw ApiException.ItemNotFound($"thumbnail {id} ({actualSize})");
            }

            // item id 是内容哈希，缩略图内容永不变，客户端可永久缓存
            ctx.Response.Headers.CacheControl = "public, max-age=31536000, immutable";
            return TypedResults.File(new FileStream(file, FileMode.Open, FileAccess.Read, FileShare.Read), "image/webp");
        });

        // 原图：预览浮层用。<img> 无法带请求头，token 走查询参数（AuthMiddleware 放行）
        group.MapGet("/file", (string id, HttpContext ctx, ItemIndex index, LibraryPaths paths) =>
        {
            var item = index.Get(id) ?? throw ApiException.ItemNotFound(id);
            var file = SourceFile(item, paths) ?? throw ApiException.InvalidParam("item 没有可用的文件位置");
            if (!File.Exists(file))
            {
                throw ApiException.ItemNotFound($"file {id}");
            }

            // item id 是内容哈希，文件内容永不变，客户端可永久缓存
            ctx.Response.Headers.CacheControl = "public, max-age=31536000, immutable";
            var contentType = new FileExtensionContentTypeProvider().TryGetContentType(file, out var ct)
                ? ct
                : "application/octet-stream";
            return TypedResults.File(new FileStream(file, FileMode.Open, FileAccess.Read, FileShare.Read), contentType);
        });

        group.MapPost("/refresh_thumbnail", async (ItemRefreshThumbnailRequest req, ItemIndex index, ThumbnailService thumbnails, LibraryConfig config, LibraryPaths paths) =>
        {
            var item = index.Get(req.Id) ?? throw ApiException.ItemNotFound(req.Id);
            var source = SourceFile(item, paths) ?? throw ApiException.InvalidParam("item 没有可用的文件位置");
            await thumbnails.GenerateAsync(req.Id, source, config.Current.ThumbnailSizes, force: true);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });
    }

    // ---------- add ----------

    private static async Task<IResult> AddAsync(ItemAddRequest req, LibraryPaths paths, ItemIndex index, IndexPipeline pipeline, CancellationToken ct)
    {
        if (req.Path is null && req.Url is null && req.ImgBase64 is null)
        {
            throw ApiException.InvalidParam("path、url、img_base64 必须提供其一");
        }

        var folderRel = req.FolderPath ?? "";
        if (folderRel != "" && !LibraryPaths.IsValidLibraryPath(folderRel))
        {
            throw ApiException.InvalidParam($"非法文件夹路径: {req.FolderPath}");
        }

        // 导入时目标目录不存在则自动创建
        var folderAbs = folderRel == "" ? paths.Root : paths.ToAbsolute(folderRel)!;
        Directory.CreateDirectory(folderAbs);

        // 获取内容来源：本地文件直接引用，url/base64 先落临时文件
        string? tempFile = null;
        string sourceAbs;
        string ext;
        string defaultName;
        try
        {
            if (req.Path is not null)
            {
                var source = new FileInfo(req.Path);
                if (!source.Exists)
                {
                    throw ApiException.InvalidParam($"文件不存在: {req.Path}");
                }

                sourceAbs = source.FullName;
                ext = Path.GetExtension(source.Name).TrimStart('.').ToLowerInvariant();
                defaultName = Path.GetFileNameWithoutExtension(source.Name);
            }
            else if (req.Url is not null)
            {
                if (!Uri.TryCreate(req.Url, UriKind.Absolute, out var uri))
                {
                    throw ApiException.InvalidParam($"非法 URL: {req.Url}");
                }

                tempFile = Path.GetTempFileName();
                using (var input = await Http.GetStreamAsync(uri, ct))
                {
                    await using var output = File.Create(tempFile);
                    await input.CopyToAsync(output, ct);
                }

                sourceAbs = tempFile;
                defaultName = WebUtility.UrlDecode(uri.Segments.LastOrDefault()?.Trim('/') ?? "");
                ext = Path.GetExtension(defaultName).TrimStart('.').ToLowerInvariant();
                if (ext == "")
                {
                    ext = ThumbnailService.DetectExtension(tempFile)
                        ?? throw ApiException.InvalidParam("无法确定文件扩展名");
                }

                if (Path.GetFileNameWithoutExtension(defaultName) is { Length: > 0 } stem)
                {
                    defaultName = stem;
                }
                else
                {
                    defaultName = "download";
                }
            }
            else
            {
                byte[] bytes;
                try
                {
                    bytes = Convert.FromBase64String(req.ImgBase64!);
                }
                catch (FormatException)
                {
                    throw ApiException.InvalidParam("img_base64 不是合法的 Base64 数据");
                }

                ext = ThumbnailService.DetectExtension(bytes)
                    ?? throw ApiException.UnsupportedFormat("无法识别的图像数据");
                tempFile = Path.GetTempFileName();
                await File.WriteAllBytesAsync(tempFile, bytes, ct);
                sourceAbs = tempFile;
                defaultName = "image";
            }

            var name = req.Name ?? defaultName;
            if (!LibraryFs.IsValidName(name))
            {
                throw ApiException.InvalidParam($"非法文件名: {req.Name}");
            }

            var fileName = ext == "" ? name : $"{name}.{ext}";
            var targetRel = folderRel == "" ? fileName : $"{folderRel}/{fileName}";
            var targetAbs = paths.ToAbsolute(targetRel)!;
            if (File.Exists(targetAbs))
            {
                throw ApiException.FileExists(targetRel);
            }

            // 先算哈希判断内容是否已存在（already_existed 语义以写入前为准）
            var hash = ContentHash.HashFile(sourceAbs, ct);
            var alreadyExisted = index.Get(hash) is not null;

            if (req.Path is not null)
            {
                File.Copy(sourceAbs, targetAbs);
            }
            else
            {
                File.Move(sourceAbs, targetAbs);
                tempFile = null;
            }

            // 哈希已算好，流水线跳过重算，避免大文件导入时二次读盘
            var result = await pipeline.SubmitUpsertAsync(targetAbs, hash);
            if (result is null)
            {
                throw new ApiException(ErrorCodes.Internal, "索引失败", StatusCodes.Status500InternalServerError);
            }

            // 附带的素材参数写入元数据；url 下载时来源网址自动记录为 Item.url
            if (req.Tags is not null || req.Annotation is not null || req.Url is not null)
            {
                await pipeline.SubmitMetadataAsync(hash, meta =>
                {
                    if (req.Tags is not null)
                    {
                        meta.Tags = req.Tags.ToList();
                    }

                    if (req.Annotation is not null)
                    {
                        meta.Annotation = req.Annotation;
                    }

                    if (req.Url is not null)
                    {
                        meta.Url = req.Url;
                    }
                });
            }

            var item = index.Get(hash)!;
            return TypedResults.Ok(Envelope<ItemAddResponse>.Ok(new ItemAddResponse(item.ToDto(trashView: false), alreadyExisted)));
        }
        finally
        {
            if (tempFile is not null && File.Exists(tempFile))
            {
                File.Delete(tempFile);
            }
        }
    }

    // ---------- update ----------

    private static async Task<IResult> UpdateAsync(ItemUpdateRequest req, LibraryPaths paths, ItemIndex index, IndexPipeline pipeline)
    {
        var item = index.Get(req.Id) ?? throw ApiException.ItemNotFound(req.Id);
        var loc = FindLocation(item, req.Path, wantTrash: null)
            ?? throw ApiException.ItemNotFound(req.Path ?? req.Id);

        if (loc.InTrash && (req.Name is not null || req.FolderPath is not null))
        {
            throw ApiException.InvalidParam("回收站中的文件不支持改名/移动，请先恢复");
        }

        if (req.Name is not null)
        {
            if (!LibraryFs.IsValidName(req.Name))
            {
                throw ApiException.InvalidParam($"非法文件名: {req.Name}");
            }

            var ext = LibraryPaths.ExtOf(loc.LibraryPath);
            var fileName = ext == "" ? req.Name : $"{req.Name}.{ext}";
            var dir = LibraryPaths.DirOf(loc.Path);
            var targetRel = dir == "" ? fileName : $"{dir}/{fileName}";
            if (targetRel != loc.Path)
            {
                var sourceAbs = paths.ToAbsolute(loc.Path)!;
                var targetAbs = paths.ToAbsolute(targetRel)!;
                if (File.Exists(targetAbs))
                {
                    throw ApiException.FileExists(targetRel);
                }

                File.Move(sourceAbs, targetAbs);
                await pipeline.SubmitMoveAsync(sourceAbs, targetAbs);
            }
        }

        if (req.FolderPath is not null)
        {
            var folderAbs = req.FolderPath == "" ? paths.Root : null;
            if (req.FolderPath != "")
            {
                if (!LibraryPaths.IsValidLibraryPath(req.FolderPath))
                {
                    throw ApiException.InvalidParam($"非法文件夹路径: {req.FolderPath}");
                }

                folderAbs = paths.ToAbsolute(req.FolderPath)!;
                if (!Directory.Exists(folderAbs))
                {
                    throw ApiException.FolderNotFound(req.FolderPath);
                }
            }

            var fileName = loc.Path[(loc.Path.LastIndexOf('/') + 1)..];
            var targetAbs = Path.Combine(folderAbs!, fileName);
            var sourceAbs2 = paths.ToAbsolute(loc.Path)!;
            if (!string.Equals(targetAbs, sourceAbs2, StringComparison.Ordinal))
            {
                if (File.Exists(targetAbs))
                {
                    throw ApiException.FileExists(req.FolderPath == "" ? fileName : $"{req.FolderPath}/{fileName}");
                }

                File.Move(sourceAbs2, targetAbs);
                await pipeline.SubmitMoveAsync(sourceAbs2, targetAbs);
            }
        }

        if (req.Star is < 0 or > 5)
        {
            throw ApiException.InvalidParam("star 取值范围为 0-5");
        }

        if (req.Tags is not null || req.Star is not null || req.Categories is not null || req.Annotation is not null || req.Url is not null)
        {
            string[]? categories = null;
            if (req.Categories is not null)
            {
                categories = req.Categories.Select(CategoryPath.Normalize).ToArray()!;
                if (categories.Any(c => c is null))
                {
                    throw ApiException.InvalidParam("包含非法分类路径");
                }

                categories = categories.Distinct(StringComparer.Ordinal).ToArray();
            }

            await pipeline.SubmitMetadataAsync(req.Id, meta =>
            {
                if (req.Tags is not null)
                {
                    meta.Tags = req.Tags.ToList();
                }

                if (req.Categories is not null)
                {
                    meta.Categories = categories!.ToList();
                }

                if (req.Star is not null)
                {
                    meta.Star = req.Star.Value;
                }

                if (req.Annotation is not null)
                {
                    meta.Annotation = req.Annotation;
                }

                if (req.Url is not null)
                {
                    meta.Url = req.Url;
                }
            });
        }

        var updated = index.Get(req.Id)!;
        return TypedResults.Ok(Envelope<ItemDto>.Ok(updated.ToDto(trashView: !updated.HasLibraryLocations)));
    }

    // ---------- delete / restore ----------

    private static async Task<IResult> DeleteAsync(ItemIdRequest req, LibraryPaths paths, ItemIndex index, IndexPipeline pipeline)
    {
        var item = index.Get(req.Id) ?? throw ApiException.ItemNotFound(req.Id);
        var loc = FindLocation(item, req.Path, wantTrash: false)
            ?? throw ApiException.InvalidParam(req.Path is null ? "item 不在库内" : $"库内不存在该文件位置: {req.Path}");

        var sourceAbs = paths.ToAbsolute(loc.Path)!;
        var trashAbs = LibraryFs.FindFreeTrashPath(paths, loc.Path, isDirectory: false);
        LibraryFs.EnsureParentDir(trashAbs);
        File.Move(sourceAbs, trashAbs);
        await pipeline.SubmitMoveAsync(sourceAbs, trashAbs);
        return TypedResults.Ok(new Envelope<object>("success", null));
    }

    private static async Task<IResult> RestoreAsync(ItemIdRequest req, LibraryPaths paths, ItemIndex index, IndexPipeline pipeline)
    {
        var item = index.Get(req.Id) ?? throw ApiException.ItemNotFound(req.Id);
        var loc = FindLocation(item, req.Path, wantTrash: true)
            ?? throw ApiException.InvalidParam(req.Path is null ? "item 不在回收站" : $"回收站中不存在该文件位置: {req.Path}");

        // 按原路径放回（回收站中的实际名称去掉 .hawk/trash/ 前缀）
        var targetAbs = paths.ToAbsolute(loc.LibraryPath)!;
        if (File.Exists(targetAbs))
        {
            throw ApiException.FileExists(loc.LibraryPath);
        }

        var sourceAbs = paths.ToAbsolute(loc.Path)!;
        LibraryFs.EnsureParentDir(targetAbs);
        File.Move(sourceAbs, targetAbs);
        await pipeline.SubmitMoveAsync(sourceAbs, targetAbs);
        return TypedResults.Ok(new Envelope<object>("success", null));
    }

    /// <summary>
    /// 定位操作的文件位置：缺省为主位置（wantTrash=false 时取首个库内位置，true 时取首个回收站位置）；
    /// 指定 path 时按视图匹配（回收站位置以其原库内路径匹配）。
    /// </summary>
    private static ItemLocation? FindLocation(Item item, string? path, bool? wantTrash)
    {
        if (path is null)
        {
            return wantTrash switch
            {
                false => item.Locations.FirstOrDefault(l => !l.InTrash),
                true => item.Locations.FirstOrDefault(l => l.InTrash),
                null => item.Locations.FirstOrDefault(l => !l.InTrash) ?? item.Locations.FirstOrDefault(),
            };
        }

        return item.Locations.FirstOrDefault(l =>
            (wantTrash is null || l.InTrash == wantTrash) &&
            (l.Path == path || l.LibraryPath == path));
    }

    /// <summary>取一个可读的源文件绝对路径（优先库内位置）</summary>
    private static string? SourceFile(Item item, LibraryPaths paths)
    {
        var loc = item.Locations.FirstOrDefault(l => !l.InTrash) ?? item.Locations.FirstOrDefault();
        return loc is null ? null : paths.ToAbsolute(loc.Path);
    }
}

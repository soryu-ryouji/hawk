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
        /// <summary>为 true 时文件夹只精确匹配直接位于该目录下的 item(不含子目录);空字符串表示库根目录</summary>
        public bool FoldersExact { get; init; }
        public string[]? Categories { get; init; }
        public string? CategoriesMatch { get; init; }
        public string[]? ExcludeCategories { get; init; }
        public string[]? ExcludeTags { get; init; }
        /// <summary>只返回未分类(没有任何分类)的 item</summary>
        public bool WithoutCategories { get; init; }
        /// <summary>只返回未标签(没有任何标签)的 item</summary>
        public bool WithoutTags { get; init; }
        public string? Ext { get; init; }
        public string? Annotation { get; init; }
        public string? Url { get; init; }
        public string? Color { get; init; }
        public bool InTrash { get; init; }
        public string? OrderBy { get; init; }
        public string? Order { get; init; }
        public int Offset { get; init; }
        public int Limit { get; init; } = 50;
    }

    public sealed record ItemListResponse(ItemDto[] Items, int Total, long TotalSize, int Offset, int Limit);

    /// <summary>虚拟网格骨架:全量 dim(与 item/list 同过滤同排序、不分页),前端据此建立完整布局后按 offset 取窗口
    /// </summary>
    public sealed record ItemSkeletonResponse(ItemSkeletonDto[] Items, long TotalSize);

    public sealed record ItemAddRequest
    {
        public string? Path { get; init; }
        public string? Url { get; init; }
        public string? ImgBase64 { get; init; }
        public string? Name { get; init; }
        public string? FolderPath { get; init; }
        public string[]? Tags { get; init; }
        public string[]? Categories { get; init; }
        public string? Annotation { get; init; }
        /// <summary>来源网页(收集场景:图片所在的页面地址),记录为 Item.url;与下载用的 url 区分</summary>
        public string? Website { get; init; }
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

    /// <summary>批量更新(item/batch_update):标签/分类为并集追加,标量与文件夹为设置;部分失败逐项跳过</summary>
    public sealed record ItemBatchUpdateRequest
    {
        public required string[] Ids { get; init; }
        public string[]? AddTags { get; init; }
        public string[]? AddCategories { get; init; }
        public int? Star { get; init; }
        public string? FolderPath { get; init; }
    }

    /// <summary>Updated=元数据实际应用数;MissingIds=内容不存在或移动冲突的 id(其余字段照常应用,除非 id 不存在)</summary>
    public sealed record ItemBatchUpdateResponse(int Updated, string[] MissingIds);

    public sealed record ItemIdRequest(string Id, string? Path);
    public sealed record ItemRefreshThumbnailRequest(string Id);

    /// <summary>内容替换(item/replace):客户端编辑(旋转/裁切等)后的新内容;编辑计算在客户端,server 只做存储层校验与写盘</summary>
    public sealed record ItemReplaceRequest
    {
        public required string Id { get; init; }
        public string? Path { get; init; }
        public required string ImgBase64 { get; init; }
    }

    private static readonly HttpClient Http = new() { Timeout = TimeSpan.FromSeconds(30) };

    public static void MapItemEndpoints(this IEndpointRouteBuilder app)
    {
        var group = app.MapGroup("/api/v1/item").WithTags("item");

        group.MapPost("/list", (ItemListRequest? req, ItemIndex index) =>
        {
            var query = BuildQuery(req);
            var items = index.Query(query, out var total, out var totalSize);
            var response = new ItemListResponse(items.ToArray(), total, totalSize, query.Offset, query.Limit);
            return TypedResults.Ok(Envelope<ItemListResponse>.Ok(response));
        });

        // 骨架:与 /list 完全相同的过滤与排序(确定性次序),不分页,只回布局所需的最低字段。
        // 前端虚拟网格用它一次性算出全部内容的总高(滚动条可自由拖动),视口内再按 offset 用 /list 取详情。
        group.MapPost("/skeleton", (ItemListRequest? req, ItemIndex index) =>
        {
            var query = BuildQuery(req);
            var items = index.QuerySkeleton(query, out var totalSize);
            return TypedResults.Ok(Envelope<ItemSkeletonResponse>.Ok(new ItemSkeletonResponse(items.ToArray(), totalSize)));
        });

        group.MapGet("/detail", (string id, ItemIndex index) =>
        {
            var dto = index.GetDto(id) ?? throw ApiException.ItemNotFound(id);
            return TypedResults.Ok(Envelope<ItemDto>.Ok(dto));
        });

        group.MapGet("/count", (ItemIndex index) => TypedResults.Ok(Envelope<int>.Ok(index.Count())));

        group.MapPost("/add", AddAsync);
        group.MapPost("/update", UpdateAsync);
        group.MapPost("/batch_update", BatchUpdateAsync);
        group.MapPost("/delete", DeleteAsync);
        group.MapPost("/restore", RestoreAsync);
        group.MapPost("/replace", ReplaceAsync);

        group.MapGet("/thumbnail", (string id, int? size, HttpContext ctx, ItemIndex index, ThumbnailService thumbnails, LibraryConfig config) =>
        {
            var actualSize = size ?? 256;
            if (!config.Current.ThumbnailSizes.Contains(actualSize))
            {
                throw ApiException.InvalidParam($"不支持的缩略图尺寸: {actualSize}");
            }

            if (!index.Contains(id))
            {
                throw ApiException.ItemNotFound(id);
            }

            var file = thumbnails.GetPath(id, actualSize);
            if (!File.Exists(file))
            {
                // 首次索引完成前缩略图可能尚未生成
                throw ApiException.ItemNotFound($"thumbnail {id} ({actualSize})");
            }

            // item id 是内容哈希,缩略图内容永不变,客户端可永久缓存
            ctx.Response.Headers.CacheControl = "public, max-age=31536000, immutable";
            return TypedResults.File(new FileStream(file, FileMode.Open, FileAccess.Read, FileShare.Read), "image/webp");
        });

        // 原图:预览浮层用。<img> 无法带请求头,token 走查询参数(AuthMiddleware 放行)
        group.MapGet("/file", (string id, HttpContext ctx, ItemIndex index, LibraryPaths paths) =>
        {
            var file = index.MainSourceAbs(id, paths) ?? throw ApiException.ItemNotFound(id);
            if (!File.Exists(file))
            {
                throw ApiException.ItemNotFound($"file {id}");
            }

            // item id 是内容哈希,文件内容永不变,客户端可永久缓存
            ctx.Response.Headers.CacheControl = "public, max-age=31536000, immutable";
            var contentType = new FileExtensionContentTypeProvider().TryGetContentType(file, out var ct)
                ? ct
                : "application/octet-stream";
            return TypedResults.File(new FileStream(file, FileMode.Open, FileAccess.Read, FileShare.Read), contentType);
        });

        group.MapPost("/refresh_thumbnail", async (ItemRefreshThumbnailRequest req, ItemIndex index, ThumbnailService thumbnails, LibraryConfig config, LibraryPaths paths) =>
        {
            var source = index.MainSourceAbs(req.Id, paths) ?? throw ApiException.ItemNotFound(req.Id);
            await thumbnails.GenerateAsync(req.Id, source, config.Current.ThumbnailSizes, force: true);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });
    }

    // ---------- list/skeleton 共用查询构造 ----------

    /// <summary>ItemListRequest → ItemQuery:/list 与 /skeleton 必须走同一条路径,保证两次查询次序逐位一致</summary>
    private static ItemQuery BuildQuery(ItemListRequest? req)
    {
        req ??= new ItemListRequest();
        LabColor? color = null;
        if (req.Color is not null)
        {
            if (ColorMath.ParseHex(req.Color) is not { } rgb)
            {
                throw ApiException.InvalidParam($"非法颜色值: {req.Color}");
            }

            color = ColorMath.RgbToLab(rgb.R, rgb.G, rgb.B);
        }

        return new ItemQuery
        {
            Ids = req.Ids, Keywords = req.Keywords, Tags = req.Tags, Star = req.Star,
            Folders = req.Folders, FoldersExact = req.FoldersExact, Categories = req.Categories, CategoriesMatch = req.CategoriesMatch,
            ExcludeCategories = req.ExcludeCategories, ExcludeTags = req.ExcludeTags,
            WithoutCategories = req.WithoutCategories, WithoutTags = req.WithoutTags,
            Ext = req.Ext, Annotation = req.Annotation, Url = req.Url, Color = color,
            InTrash = req.InTrash, OrderBy = req.OrderBy, Order = req.Order,
            Offset = req.Offset, Limit = req.Limit,
        };
    }

    // ---------- replace ----------

    /// <summary>
    /// 内容替换(item/replace):客户端编辑(旋转/裁切等)后的新内容提交存储层。
    /// 哈希变化 → id 漂移,元数据继承迁移/事件/缩略图重建由索引流水线闭环。
    /// </summary>
    private static async Task<IResult> ReplaceAsync(ItemReplaceRequest req, LibraryPaths paths, ItemIndex index, IndexPipeline pipeline, CancellationToken ct)
    {
        var loc = index.FindLocation(req.Id, req.Path, wantTrash: null)
            ?? throw ApiException.ItemNotFound(req.Path ?? req.Id);
        if (loc.InTrash)
        {
            throw ApiException.InvalidParam("回收站中的文件不支持内容替换,请先恢复");
        }

        byte[] bytes;
        try
        {
            bytes = Convert.FromBase64String(req.ImgBase64);
        }
        catch (FormatException)
        {
            throw ApiException.InvalidParam("img_base64 不是合法的 Base64 数据");
        }

        // 内容必须是图像且格式与文件扩展名一致:扩展名与内容错位会破坏类型推断与预览
        var fileExt = LibraryPaths.ExtOf(loc.LibraryPath);
        var ext = ThumbnailService.DetectExtension(bytes)
            ?? throw ApiException.UnsupportedFormat("无法识别的图像数据");
        if (ext != fileExt)
        {
            throw ApiException.UnsupportedFormat($"图像格式({ext})与文件扩展名({fileExt})不一致");
        }

        var hash = ContentHash.HashBytes(bytes);
        if (hash == req.Id)
        {
            // 内容未变化(幂等):不触发漂移,直接返回当前投影
            var unchanged = index.GetDto(req.Id) ?? throw ApiException.ItemNotFound(req.Id);
            return TypedResults.Ok(Envelope<ItemDto>.Ok(unchanged));
        }

        // 写回原路径。保留原修改时间:旋转等修正性编辑不改变素材的时序位置
        // (mtime 是排序与哈希复用判定的依据);创建时间由截断重写天然保留。
        var targetAbs = paths.ToAbsolute(loc.Path)!;
        var lastWriteUtc = File.GetLastWriteTimeUtc(targetAbs);
        await File.WriteAllBytesAsync(targetAbs, bytes, ct);
        File.SetLastWriteTimeUtc(targetAbs, lastWriteUtc);

        var result = await pipeline.SubmitUpsertAsync(targetAbs, hash);
        if (result is null)
        {
            throw new ApiException(ErrorCodes.Internal, "索引失败", StatusCodes.Status500InternalServerError);
        }

        return TypedResults.Ok(Envelope<ItemDto>.Ok(result.Item));
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

        // 获取内容来源:本地文件直接引用,url/base64 先落临时文件
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

            // 先算哈希判断内容是否已存在(already_existed 语义以写入前为准)
            var hash = ContentHash.HashFile(sourceAbs, ct);
            var alreadyExisted = index.Contains(hash);

            if (req.Path is not null)
            {
                File.Copy(sourceAbs, targetAbs);
                // 保留原文件时间属性:File.Copy 会重置创建时间,修改时间在各平台行为也不可靠;
                // modification_time 排序与文件管理器观感都以原文件为准
                var sourceInfo = new FileInfo(sourceAbs);
                File.SetCreationTimeUtc(targetAbs, sourceInfo.CreationTimeUtc);
                File.SetLastWriteTimeUtc(targetAbs, sourceInfo.LastWriteTimeUtc);
            }
            else
            {
                File.Move(sourceAbs, targetAbs);
                tempFile = null;
            }

            // 哈希已算好,流水线跳过重算,避免大文件导入时二次读盘
            var result = await pipeline.SubmitUpsertAsync(targetAbs, hash);
            if (result is null)
            {
                throw new ApiException(ErrorCodes.Internal, "索引失败", StatusCodes.Status500InternalServerError);
            }

            // 附带的素材参数写入元数据;website(来源网页)记录为 Item.url,下载用的 url 不覆盖它
            if (req.Tags is not null || req.Annotation is not null || req.Website is not null || req.Categories is not null)
            {
                string[]? categories = null;
                if (req.Categories is not null)
                {
                    categories = req.Categories.Select(CategoryName.Normalize).ToArray()!;
                    if (categories.Any(c => c is null))
                    {
                        throw ApiException.InvalidParam("包含非法分类名称");
                    }

                    categories = categories.Distinct(StringComparer.Ordinal).ToArray();
                }

                await pipeline.SubmitMetadataAsync(hash, meta =>
                {
                    if (req.Tags is not null)
                    {
                        meta.Tags = req.Tags.ToList();
                    }

                    if (categories is not null)
                    {
                        meta.Categories = categories.ToList();
                    }

                    if (req.Annotation is not null)
                    {
                        meta.Annotation = req.Annotation;
                    }

                    if (req.Website is not null)
                    {
                        meta.Url = req.Website;
                    }
                });
            }

            // 元数据可能刚经 SubmitMetadataAsync 更新,响应以最新投影为准(锁内投影,锁外不持有 Item)
            var dto = index.GetDto(hash) ?? throw new ApiException(ErrorCodes.Internal, "索引失败", StatusCodes.Status500InternalServerError);
            return TypedResults.Ok(Envelope<ItemAddResponse>.Ok(new ItemAddResponse(dto, alreadyExisted)));
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
        var loc = index.FindLocation(req.Id, req.Path, wantTrash: null)
            ?? throw ApiException.ItemNotFound(req.Path ?? req.Id);

        if (loc.InTrash && (req.Name is not null || req.FolderPath is not null))
        {
            throw ApiException.InvalidParam("回收站中的文件不支持改名/移动,请先恢复");
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

            // name 分支可能已移动过文件:按移动后的最新位置再移动(改名+移动同请求时基于新文件名计算目标)
            var current = index.FindLocation(req.Id, req.Path, wantTrash: null) ?? loc;
            var fileName = current.Path[(current.Path.LastIndexOf('/') + 1)..];
            var targetAbs = Path.Combine(folderAbs!, fileName);
            var sourceAbs2 = paths.ToAbsolute(current.Path)!;
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
                categories = req.Categories.Select(CategoryName.Normalize).ToArray()!;
                if (categories.Any(c => c is null))
                {
                    throw ApiException.InvalidParam("包含非法分类名称");
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

        var updated = index.GetDto(req.Id) ?? throw ApiException.ItemNotFound(req.Id);
        return TypedResults.Ok(Envelope<ItemDto>.Ok(updated));
    }

    // ---------- batch_update ----------

    private static async Task<IResult> BatchUpdateAsync(ItemBatchUpdateRequest req, LibraryPaths paths, ItemIndex index, IndexPipeline pipeline)
    {
        if (req.Ids.Length == 0)
        {
            throw ApiException.InvalidParam("ids 不能为空");
        }

        if (req.AddTags is null && req.AddCategories is null && req.Star is null && req.FolderPath is null)
        {
            throw ApiException.InvalidParam("至少提供一个更新字段");
        }

        if (req.Star is < 0 or > 5)
        {
            throw ApiException.InvalidParam("star 取值范围为 0-5");
        }

        string[]? addCategories = null;
        if (req.AddCategories is not null)
        {
            addCategories = req.AddCategories.Select(CategoryName.Normalize).ToArray()!;
            if (addCategories.Any(c => c is null))
            {
                throw ApiException.InvalidParam("包含非法分类名称");
            }

            addCategories = addCategories.Distinct(StringComparer.Ordinal).ToArray();
        }

        var ids = req.Ids.Distinct(StringComparer.Ordinal).ToArray();
        var moveFailed = new List<string>();

        // folder_path:逐个移动主位置(库内);已在目标处的跳过;无库内位置(全在回收站)的移动不适用,跳过
        if (req.FolderPath is not null)
        {
            string folderAbs;
            if (req.FolderPath == "")
            {
                folderAbs = paths.Root;
            }
            else
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

            foreach (var id in ids)
            {
                var loc = index.FindLocation(id, null, wantTrash: false);
                if (loc is null)
                {
                    continue;
                }

                var fileName = loc.Path[(loc.Path.LastIndexOf('/') + 1)..];
                var sourceAbs = paths.ToAbsolute(loc.Path)!;
                var targetAbs = Path.Combine(folderAbs, fileName);
                if (string.Equals(targetAbs, sourceAbs, StringComparison.Ordinal))
                {
                    continue;
                }

                // 同名冲突不整体失败:跳过该项移动并记入 missing,其余照常
                if (File.Exists(targetAbs))
                {
                    moveFailed.Add(id);
                    continue;
                }

                try
                {
                    File.Move(sourceAbs, targetAbs);
                }
                catch (IOException)
                {
                    moveFailed.Add(id);
                    continue;
                }

                await pipeline.SubmitMoveAsync(sourceAbs, targetAbs);
            }
        }

        // 元数据:标签/分类并集追加、评分设置;一次提交,由流水线批量应用(单写者)
        var updated = 0;
        var missing = new List<string>(moveFailed);
        if (req.AddTags is not null || addCategories is not null || req.Star is not null)
        {
            var result = await pipeline.SubmitBatchMetadataAsync(ids, meta =>
            {
                if (req.AddTags is not null)
                {
                    meta.Tags = meta.Tags.Union(req.AddTags, StringComparer.Ordinal).ToList();
                }

                if (addCategories is not null)
                {
                    meta.Categories = meta.Categories.Union(addCategories, StringComparer.Ordinal).ToList();
                }

                if (req.Star is not null)
                {
                    meta.Star = req.Star.Value;
                }
            });
            updated = result.Updated;
            missing.AddRange(result.MissingIds);
        }

        return TypedResults.Ok(Envelope<ItemBatchUpdateResponse>.Ok(
            new ItemBatchUpdateResponse(updated, missing.Distinct(StringComparer.Ordinal).ToArray())));
    }

    // ---------- delete / restore ----------

    private static async Task<IResult> DeleteAsync(ItemIdRequest req, LibraryPaths paths, ItemIndex index, IndexPipeline pipeline)
    {
        var loc = index.FindLocation(req.Id, req.Path, wantTrash: false)
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
        var loc = index.FindLocation(req.Id, req.Path, wantTrash: true)
            ?? throw ApiException.InvalidParam(req.Path is null ? "item 不在回收站" : $"回收站中不存在该文件位置: {req.Path}");

        // 按原路径放回(回收站中的实际名称去掉 .hawk/trash/ 前缀)
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
}

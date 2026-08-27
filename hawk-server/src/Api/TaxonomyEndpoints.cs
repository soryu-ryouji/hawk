using Hawk.Server.Core;

namespace Hawk.Server.Api;

/// <summary>
/// category / tag 端点。分类是层级路径（树由路径派生），标签是扁平名字。
/// 注册表与元数据批量迁移均由索引流水线执行（单写者）；校验发生在端点层。
/// </summary>
public static class TaxonomyEndpoints
{
    public sealed record CategoryNode(string Path, string Name, CategoryNode[] Children);

    public sealed record CategoryCreateRequest(string Path);
    public sealed record CategoryUpdateRequest(string Path, string? Name, string? ParentPath);
    public sealed record CategoryPathRequest(string Path);

    public sealed record TagInfo(string Name, int Count);

    public sealed record TagCreateRequest(string Name);
    public sealed record TagUpdateRequest(string Name, string NewName);
    public sealed record TagNameRequest(string Name);

    public static void MapTaxonomyEndpoints(this IEndpointRouteBuilder app)
    {
        var category = app.MapGroup("/api/v1/category").WithTags("category");
        var tag = app.MapGroup("/api/v1/tag").WithTags("tag");

        // ---------- category ----------

        category.MapGet("/list", (CategoryRegistry registry, ItemIndex index) =>
        {
            var paths = registry.Snapshot().Union(index.AllCategories(), StringComparer.Ordinal).ToHashSet(StringComparer.Ordinal);
            var tree = new CategoryNode("", "", BuildChildren(paths, ""));
            return TypedResults.Ok(Envelope<CategoryNode>.Ok(tree));
        });

        category.MapPost("/create", async (CategoryCreateRequest req, CategoryRegistry registry, ItemIndex index, IndexPipeline pipeline) =>
        {
            var path = CategoryPath.Normalize(req.Path)
                ?? throw ApiException.InvalidParam($"非法分类路径: {req.Path}");
            if (CategoryExists(registry, index, path))
            {
                throw ApiException.CategoryExists(path);
            }

            await pipeline.SubmitCategoryCreateAsync(path);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });

        category.MapPost("/update", async (CategoryUpdateRequest req, CategoryRegistry registry, ItemIndex index, IndexPipeline pipeline) =>
        {
            var path = CategoryPath.Normalize(req.Path)
                ?? throw ApiException.InvalidParam($"非法分类路径: {req.Path}");
            if (!CategoryExists(registry, index, path))
            {
                throw ApiException.CategoryNotFound(path);
            }

            string? parentPath = null;
            if (req.ParentPath is not null)
            {
                if (req.ParentPath == "")
                {
                    parentPath = ""; // 移到根级
                }
                else
                {
                    parentPath = CategoryPath.Normalize(req.ParentPath)
                        ?? throw ApiException.InvalidParam($"非法父分类路径: {req.ParentPath}");
                }
            }

            if (req.Name is not null && !LibraryFs.IsValidName(req.Name))
            {
                throw ApiException.InvalidParam($"非法分类名称: {req.Name}");
            }

            var newName = req.Name ?? CategoryPath.NameOf(path);
            var newParent = parentPath ?? CategoryPath.ParentOf(path);
            var newPath = newParent == "" ? newName : newParent + "/" + newName;
            if (newPath != path)
            {
                if (CategoryPath.IsSameOrDescendant(newPath, path))
                {
                    throw ApiException.InvalidParam("不能移动到自身子分类");
                }

                if (CategoryExists(registry, index, newPath))
                {
                    throw ApiException.CategoryExists(newPath);
                }
            }

            await pipeline.SubmitCategoryUpdateAsync(path, req.Name, parentPath);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });

        category.MapPost("/delete", async (CategoryPathRequest req, CategoryRegistry registry, ItemIndex index, IndexPipeline pipeline) =>
        {
            var path = CategoryPath.Normalize(req.Path)
                ?? throw ApiException.InvalidParam($"非法分类路径: {req.Path}");
            if (!CategoryExists(registry, index, path))
            {
                throw ApiException.CategoryNotFound(path);
            }

            await pipeline.SubmitCategoryDeleteAsync(path);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });

        // ---------- tag ----------

        tag.MapGet("/list", (TagRegistry registry, ItemIndex index) =>
        {
            var counts = index.TagsWithCounts().ToDictionary(t => t.Name, t => t.Count, StringComparer.Ordinal);
            var names = registry.Snapshot().Union(counts.Keys, StringComparer.Ordinal)
                .OrderBy(n => n, StringComparer.OrdinalIgnoreCase)
                .Select(n => new TagInfo(n, counts.GetValueOrDefault(n)))
                .ToArray();
            return TypedResults.Ok(Envelope<TagInfo[]>.Ok(names));
        });

        tag.MapPost("/create", async (TagCreateRequest req, IndexPipeline pipeline) =>
        {
            var name = NormalizeTag(req.Name);
            await pipeline.SubmitTagCreateAsync(name);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });

        tag.MapPost("/update", async (TagUpdateRequest req, TagRegistry registry, ItemIndex index, IndexPipeline pipeline) =>
        {
            var name = NormalizeTag(req.Name);
            if (!TagExists(registry, index, name))
            {
                throw ApiException.TagNotFound(name);
            }

            var newName = NormalizeTag(req.NewName);
            if (newName == name)
            {
                return TypedResults.Ok(new Envelope<object>("success", null));
            }

            await pipeline.SubmitTagUpdateAsync(name, newName);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });

        tag.MapPost("/delete", async (TagNameRequest req, TagRegistry registry, ItemIndex index, IndexPipeline pipeline) =>
        {
            var name = NormalizeTag(req.Name);
            if (!TagExists(registry, index, name))
            {
                throw ApiException.TagNotFound(name);
            }

            await pipeline.SubmitTagDeleteAsync(name);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });
    }

    /// <summary>分类存在性：注册表 ∪ 全部 item 赋值（含回收站）</summary>
    private static bool CategoryExists(CategoryRegistry registry, ItemIndex index, string path) =>
        registry.Contains(path) || index.AllCategories().Contains(path, StringComparer.Ordinal);

    private static bool TagExists(TagRegistry registry, ItemIndex index, string name) =>
        registry.Contains(name) || index.TagsWithCounts().Any(t => t.Name == name);

    private static string NormalizeTag(string name)
    {
        var trimmed = name.Trim();
        return trimmed == "" ? throw ApiException.InvalidParam("标签名称不能为空") : trimmed;
    }

    private static CategoryNode[] BuildChildren(HashSet<string> paths, string prefix) =>
        paths.Where(p => CategoryPath.ParentOf(p) == prefix)
            .OrderBy(p => p, StringComparer.OrdinalIgnoreCase)
            .Select(p => new CategoryNode(p, CategoryPath.NameOf(p), BuildChildren(paths, p)))
            .ToArray();
}

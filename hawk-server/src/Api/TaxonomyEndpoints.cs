using Hawk.Server.Core;

namespace Hawk.Server.Api;

/// <summary>
/// category / tag 端点。分类与标签同为扁平名字（item 可同时挂多个）；
/// 区别：标签完全自由，分类需先创建（可空挂），用于受控词表。
/// 注册表与元数据批量迁移均由索引流水线执行（单写者）；校验发生在端点层。
/// </summary>
public static class TaxonomyEndpoints
{
    public sealed record CategoryInfo(string Name, int Count);

    public sealed record CategoryCreateRequest(string Name);
    public sealed record CategoryUpdateRequest(string Name, string NewName);
    public sealed record CategoryNameRequest(string Name);

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
            var counts = index.CategoryCounts();
            var names = registry.Snapshot().Union(counts.Keys, StringComparer.Ordinal)
                .OrderBy(n => n, StringComparer.OrdinalIgnoreCase)
                .Select(n => new CategoryInfo(n, counts.GetValueOrDefault(n)))
                .ToArray();
            return TypedResults.Ok(Envelope<CategoryInfo[]>.Ok(names));
        });

        category.MapPost("/create", async (CategoryCreateRequest req, CategoryRegistry registry, ItemIndex index, IndexPipeline pipeline) =>
        {
            var name = CategoryName.Normalize(req.Name)
                ?? throw ApiException.InvalidParam($"非法分类名称: {req.Name}");
            if (CategoryExists(registry, index, name))
            {
                throw ApiException.CategoryExists(name);
            }

            await pipeline.SubmitCategoryCreateAsync(name);
            return TypedResults.Ok(new Envelope<object>("success", null));
        });

        category.MapPost("/update", async (CategoryUpdateRequest req, CategoryRegistry registry, ItemIndex index, IndexPipeline pipeline) =>
        {
            var name = CategoryName.Normalize(req.Name)
                ?? throw ApiException.InvalidParam($"非法分类名称: {req.Name}");
            if (!CategoryExists(registry, index, name))
            {
                throw ApiException.CategoryNotFound(name);
            }

            var newName = CategoryName.Normalize(req.NewName)
                ?? throw ApiException.InvalidParam($"非法分类名称: {req.NewName}");
            if (newName != name)
            {
                await pipeline.SubmitCategoryUpdateAsync(name, newName);
            }

            return TypedResults.Ok(new Envelope<object>("success", null));
        });

        category.MapPost("/delete", async (CategoryNameRequest req, CategoryRegistry registry, ItemIndex index, IndexPipeline pipeline) =>
        {
            var name = CategoryName.Normalize(req.Name)
                ?? throw ApiException.InvalidParam($"非法分类名称: {req.Name}");
            if (!CategoryExists(registry, index, name))
            {
                throw ApiException.CategoryNotFound(name);
            }

            await pipeline.SubmitCategoryDeleteAsync(name);
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
    private static bool CategoryExists(CategoryRegistry registry, ItemIndex index, string name) =>
        registry.Contains(name) || index.AllCategories().Contains(name, StringComparer.Ordinal);

    private static bool TagExists(TagRegistry registry, ItemIndex index, string name) =>
        registry.Contains(name) || index.TagsWithCounts().Any(t => t.Name == name);

    private static string NormalizeTag(string name)
    {
        var trimmed = name.Trim();
        return trimmed == "" ? throw ApiException.InvalidParam("标签名称不能为空") : trimmed;
    }
}

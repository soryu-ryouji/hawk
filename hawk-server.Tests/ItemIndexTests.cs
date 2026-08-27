using Hawk.Server.Core;

namespace Hawk.Server.Tests;

public class ItemIndexTests
{
    private readonly ItemIndex _index = new();

    private Item AddItem(string hash, string location, string[]? tags = null, int star = 0,
        string? annotation = null, string? url = null, long size = 100, long mtime = 1000, string[]? categories = null)
    {
        var item = _index.GetOrAdd(hash, out _);
        if (tags is not null)
        {
            item.Tags.AddRange(tags);
        }

        if (categories is not null)
        {
            item.Categories.AddRange(categories);
        }

        item.Star = star;
        item.Annotation = annotation;
        item.Url = url;
        _index.AddOrUpdateLocation(hash, location, size, mtime);
        return item;
    }

    private ItemIndexTests Seed()
    {
        AddItem("h1", "posters/2024/sunset.jpg", tags: ["nature", "sunset"], star: 4,
            annotation: "Beautiful sunset", url: "https://example.com/1", size: 300, mtime: 3000);
        AddItem("h2", "posters/cat.png", tags: ["nature"], star: 2, size: 100, mtime: 1000);
        AddItem("h3", "icons/logo.svg", tags: ["brand"], size: 200, mtime: 2000);
        var trashed = AddItem("h4", ".hawk/trash/old/dead.jpg", tags: ["nature"], size: 50, mtime: 500);
        return this;
    }

    [Fact]
    public void 默认查询排除回收站()
    {
        Seed();
        var result = _index.Query(new ItemQuery(), out var total);
        Assert.Equal(3, total);
        Assert.Equal(3, result.Count);
    }

    [Fact]
    public void 回收站视图只含回收站item()
    {
        Seed();
        var result = _index.Query(new ItemQuery { InTrash = true }, out var total);
        Assert.Equal(1, total);
        Assert.Equal("h4", result[0].Id);
        Assert.Equal(["old/dead.jpg"], result[0].Paths);
    }

    [Fact]
    public void Count不含回收站()
    {
        Seed();
        Assert.Equal(3, _index.Count());
    }

    [Fact]
    public void 颜色过滤_相近色命中()
    {
        Seed();
        _index.SetPalette("h1", [PaletteColor.FromRgb(0x34, 0x44, 0x41, 100)]);

        // 同一颜色命中
        var hit = _index.Query(new ItemQuery { Color = ColorMath.RgbToLab(0x34, 0x44, 0x41) }, out var hitTotal);
        Assert.Equal(1, hitTotal);
        Assert.Equal("h1", hit[0].Id);

        // 阈值内的相近色命中（ΔE ≈ 6）
        _index.Query(new ItemQuery { Color = ColorMath.RgbToLab(0x2E, 0x4E, 0x44) }, out var nearTotal);
        Assert.Equal(1, nearTotal);

        // 阈值外的颜色不命中
        _index.Query(new ItemQuery { Color = ColorMath.RgbToLab(0xFF, 0x00, 0x00) }, out var farTotal);
        Assert.Equal(0, farTotal);
    }

    [Fact]
    public void 颜色过滤_无调色板不命中()
    {
        Seed();
        _index.Query(new ItemQuery { Color = ColorMath.RgbToLab(0x34, 0x44, 0x41) }, out var total);
        Assert.Equal(0, total);
    }

    [Fact]
    public void SetPalette_item不存在时返回Null()
    {
        Assert.Null(_index.SetPalette("missing", [PaletteColor.FromRgb(0, 0, 0, 100)]));
    }

    [Theory]
    [InlineData(new[] { "nature" }, 2)]
    [InlineData(new[] { "nature", "sunset" }, 1)]  // AND 语义
    [InlineData(new[] { "nature", "missing" }, 0)]
    public void 标签过滤_AND语义(string[] tags, int expected)
    {
        Seed();
        _index.Query(new ItemQuery { Tags = tags }, out var total);
        Assert.Equal(expected, total);
    }

    [Theory]
    [InlineData(new[] { "sunset" }, 1)]      // 命中名称
    [InlineData(new[] { "beautiful" }, 1)]   // 命中备注（大小写不敏感）
    [InlineData(new[] { "sunset", "beautiful" }, 1)] // 关键词 AND
    [InlineData(new[] { "sunset", "nothing" }, 0)]
    public void 关键词过滤_匹配名称与备注(string[] keywords, int expected)
    {
        Seed();
        _index.Query(new ItemQuery { Keywords = keywords }, out var total);
        Assert.Equal(expected, total);
    }

    [Theory]
    [InlineData(new[] { "posters" }, 2)]          // 含子目录
    [InlineData(new[] { "posters/2024" }, 1)]
    [InlineData(new[] { "pos" }, 0)]              // 前缀必须按目录边界匹配
    [InlineData(new[] { "icons", "posters/2024" }, 2)] // 多文件夹 OR
    [InlineData(new[] { "" }, 3)]                 // 根目录匹配全部库内 item
    public void 文件夹过滤_前缀含子目录(string[] folders, int expected)
    {
        Seed();
        _index.Query(new ItemQuery { Folders = folders }, out var total);
        Assert.Equal(expected, total);
    }

    [Fact]
    public void 其他过滤条件()
    {
        Seed();
        _index.Query(new ItemQuery { Star = 4 }, out var byStar);
        Assert.Equal(1, byStar);

        _index.Query(new ItemQuery { Ext = "png" }, out var byExt);
        Assert.Equal(1, byExt);

        _index.Query(new ItemQuery { Annotation = "sunset" }, out var byAnn);
        Assert.Equal(1, byAnn);

        _index.Query(new ItemQuery { Url = "example.com" }, out var byUrl);
        Assert.Equal(1, byUrl);

        _index.Query(new ItemQuery { Ids = ["h1", "h3"] }, out var byIds);
        Assert.Equal(2, byIds);
    }

    [Fact]
    public void 排序与分页()
    {
        Seed();
        // 默认 modification_time desc
        var desc = _index.Query(new ItemQuery(), out _);
        Assert.Equal(["h1", "h3", "h2"], desc.Select(d => d.Id).ToArray());

        var asc = _index.Query(new ItemQuery { Order = "asc", OrderBy = "size" }, out _);
        Assert.Equal(["h2", "h3", "h1"], asc.Select(d => d.Id).ToArray());

        var page = _index.Query(new ItemQuery { Offset = 1, Limit = 1 }, out var total);
        Assert.Equal(3, total);          // total 是分页前计数
        Assert.Single(page);
        Assert.Equal("h3", page[0].Id);
    }

    [Fact]
    public void 分类过滤_any与all_含子分类()
    {
        AddItem("c1", "a.png", categories: ["插画/人物"]);
        AddItem("c2", "b.png", categories: ["插画", "参考"]);
        AddItem("c3", "c.png", categories: ["摄影"]);

        // any：命中任一（"插画" 含子分类 "插画/人物"）
        _index.Query(new ItemQuery { Categories = ["插画"] }, out var any);
        Assert.Equal(2, any);

        // all：必须同时命中「插画」与「参考」
        _index.Query(new ItemQuery { Categories = ["插画", "参考"], CategoriesMatch = "all" }, out var all);
        Assert.Equal(1, all);
    }

    [Fact]
    public void 排除过滤_分类与标签()
    {
        AddItem("c1", "a.png", tags: ["nature"], categories: ["插画/人物"]);
        AddItem("c2", "b.png", tags: ["work"], categories: ["摄影"]);
        AddItem("c3", "c.png");

        _index.Query(new ItemQuery { ExcludeCategories = ["插画"] }, out var noCat);
        Assert.Equal(2, noCat);

        _index.Query(new ItemQuery { ExcludeTags = ["nature"] }, out var noTag);
        Assert.Equal(2, noTag);
    }

    [Fact]
    public void 位置增减与移动()
    {
        AddItem("h1", "a.png");
        Assert.Equal("h1", _index.HashByLocation("a.png"));

        // 同内容第二个位置
        _index.AddOrUpdateLocation("h1", "dir/b.png", 100, 1000);
        Assert.Equal(2, _index.Get("h1")!.Locations.Count);

        // 移动位置不改变 hash
        Assert.Equal("h1", _index.MoveLocation("dir/b.png", "dir2/b.png"));
        Assert.Null(_index.HashByLocation("dir/b.png"));
        Assert.Equal("h1", _index.HashByLocation("dir2/b.png"));

        // 摘掉全部位置后 item 移除
        _index.RemoveLocation("a.png");
        Assert.NotNull(_index.Get("h1"));
        _index.RemoveLocation("dir2/b.png");
        Assert.Null(_index.Get("h1"));
    }

    [Fact]
    public void LocationsUnder按目录前缀快照()
    {
        AddItem("h1", "dir/a.png");
        AddItem("h1", "dir/sub/b.png");
        AddItem("h2", "other/c.png");

        Assert.Equal(2, _index.LocationsUnder("dir/").Length);
        Assert.Empty(_index.LocationsUnder("missing/"));
    }
}

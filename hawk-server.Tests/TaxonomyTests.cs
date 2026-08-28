using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;

namespace Hawk.Server.Tests;

public class CategoryNameTests
{
    [Theory]
    [InlineData("插画", "插画")]
    [InlineData("  人物  ", "人物")] // trim
    public void Normalize_合法名称(string raw, string expected)
    {
        Assert.Equal(expected, CategoryName.Normalize(raw));
    }

    [Theory]
    [InlineData("")]
    [InlineData("  ")]
    [InlineData("插画/人物")] // 层级已废弃，含斜杠非法
    [InlineData("a\\b")]
    public void Normalize_非法名称(string raw)
    {
        Assert.Null(CategoryName.Normalize(raw));
    }
}

public class CategoryRegistryTests
{
    private readonly TempDir _dir = new();

    private (LibraryPaths paths, CategoryRegistry registry) Create()
    {
        var paths = new LibraryPaths(_dir.Root);
        paths.EnsureLayout();
        return (paths, new CategoryRegistry(paths, NullLogger<CategoryRegistry>.Instance));
    }

    [Fact]
    public void 登记去重且排序()
    {
        var (_, registry) = Create();
        registry.Register("插画");
        registry.Register("插画");
        registry.Register("参考");

        Assert.Equal(["参考", "插画"], registry.Snapshot());
    }

    [Fact]
    public void 重命名与合并()
    {
        var (_, registry) = Create();
        registry.Register("old");
        registry.Rename("old", "new");
        Assert.Equal(["new"], registry.Snapshot());

        // 目标已存在 → 合并
        registry.Register("another");
        registry.Rename("new", "another");
        Assert.Equal(["another"], registry.Snapshot());
    }

    [Fact]
    public void 删除()
    {
        var (_, registry) = Create();
        registry.Register("插画");
        registry.Register("参考");
        registry.Delete("插画");
        Assert.Equal(["参考"], registry.Snapshot());
    }

    [Fact]
    public void 持久化_重载后保持()
    {
        var (paths, registry) = Create();
        registry.Register("插画");

        var reloaded = new CategoryRegistry(paths, NullLogger<CategoryRegistry>.Instance);
        Assert.Equal(["插画"], reloaded.Snapshot());
    }
}

public class TagRegistryTests
{
    private readonly TempDir _dir = new();

    private (LibraryPaths paths, TagRegistry registry) Create()
    {
        var paths = new LibraryPaths(_dir.Root);
        paths.EnsureLayout();
        return (paths, new TagRegistry(paths, NullLogger<TagRegistry>.Instance));
    }

    [Fact]
    public void 登记去重且持久化()
    {
        var (paths, registry) = Create();
        registry.Register("nature");
        registry.Register("nature");
        registry.Register("sunset");

        Assert.Equal(["nature", "sunset"], registry.Snapshot());
        var reloaded = new TagRegistry(paths, NullLogger<TagRegistry>.Instance);
        Assert.Equal(["nature", "sunset"], reloaded.Snapshot());
    }

    [Fact]
    public void 重命名与合并()
    {
        var (_, registry) = Create();
        registry.Register("old");
        registry.Rename("old", "new");
        Assert.Equal(["new"], registry.Snapshot());

        // 目标已存在 → 合并
        registry.Register("another");
        registry.Rename("new", "another");
        Assert.Equal(["another"], registry.Snapshot());
    }

    [Fact]
    public void 删除()
    {
        var (_, registry) = Create();
        registry.Register("nature");
        registry.Delete("nature");
        Assert.Empty(registry.Snapshot());
    }
}

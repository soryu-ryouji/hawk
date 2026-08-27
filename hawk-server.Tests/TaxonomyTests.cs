using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;

namespace Hawk.Server.Tests;

public class CategoryPathTests
{
    [Theory]
    [InlineData("插画", "插画")]
    [InlineData("插画/人物", "插画/人物")]
    [InlineData("插画 / 人物 ", "插画/人物")]   // 段 trim
    [InlineData("插画//人物", "插画/人物")]      // 空段折叠
    public void Normalize_合法路径(string raw, string expected)
    {
        Assert.Equal(expected, CategoryPath.Normalize(raw));
    }

    [Theory]
    [InlineData("")]
    [InlineData("  ")]
    [InlineData("a/../b")]
    [InlineData("a/./b")]
    [InlineData("a\\b")]
    public void Normalize_非法路径(string raw)
    {
        Assert.Null(CategoryPath.Normalize(raw));
    }

    [Theory]
    [InlineData("插画/人物", "插画", true)]
    [InlineData("插画/人物", "插画", true)]
    [InlineData("插画", "插画", true)]
    [InlineData("插画集", "插画", false)]  // 前缀必须按段边界
    public void IsSameOrDescendant(string path, string prefix, bool expected)
    {
        Assert.Equal(expected, CategoryPath.IsSameOrDescendant(path, prefix));
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
    public void 登记自动补齐祖先()
    {
        var (_, registry) = Create();
        registry.Register("插画/人物/女");

        Assert.Equal(["插画", "插画/人物", "插画/人物/女"], registry.Snapshot());
    }

    [Fact]
    public void 重命名子树跟随()
    {
        var (_, registry) = Create();
        registry.Register("插画/人物");
        registry.Register("插画/场景");
        registry.Register("参考");

        registry.Rename("插画", "灵感");

        Assert.Equal(["参考", "灵感", "灵感/人物", "灵感/场景"], registry.Snapshot());
    }

    [Fact]
    public void 删除连子树一并清除()
    {
        var (_, registry) = Create();
        registry.Register("插画/人物");
        registry.Register("参考");

        registry.Delete("插画");

        Assert.Equal(["参考"], registry.Snapshot());
    }

    [Fact]
    public void 持久化_重载后保持()
    {
        var (paths, registry) = Create();
        registry.Register("插画/人物");

        var reloaded = new CategoryRegistry(paths, NullLogger<CategoryRegistry>.Instance);
        Assert.Equal(["插画", "插画/人物"], reloaded.Snapshot());
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

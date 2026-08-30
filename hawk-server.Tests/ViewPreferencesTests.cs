using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;

namespace Hawk.Server.Tests;

/// <summary>ViewPreferences（视图偏好注册表，.hawk/view.toml）单测</summary>
public class ViewPreferencesTests
{
    private readonly TempDir _dir = new();

    private (LibraryPaths paths, ViewPreferences prefs) Create()
    {
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();
        return (paths, new ViewPreferences(paths, NullLogger<ViewPreferences>.Instance));
    }

    [Fact]
    public void 设置后重载_条目完整往返()
    {
        var (paths, prefs) = Create();
        Assert.True(ViewPreferences.TryParseScope("folder:设计素材/海报", out var s1));
        Assert.True(ViewPreferences.TryParseScope("folder:", out var root));
        Assert.True(ViewPreferences.TryParseScope("tag:logo", out var s2));
        prefs.Set(s1, new ViewSort("name", "asc"));
        prefs.Set(root, new ViewSort("size", "desc"));
        prefs.Set(s2, new ViewSort("star", "desc"));

        // 模拟重启：新实例从磁盘加载
        var reloaded = new ViewPreferences(paths, NullLogger<ViewPreferences>.Instance);
        var all = reloaded.Snapshot();
        Assert.Equal(3, all.Count);
        Assert.Equal(new ViewSort("name", "asc"), all["folder:设计素材/海报"]);
        Assert.Equal(new ViewSort("size", "desc"), all["folder:"]);
        Assert.Equal(new ViewSort("star", "desc"), all["tag:logo"]);
    }

    [Fact]
    public void 删除后回到默认_文件同步移除()
    {
        var (paths, prefs) = Create();
        prefs.Set("tag:logo", new ViewSort("star", "desc"));
        prefs.Delete("tag:logo");
        Assert.Empty(prefs.Snapshot());

        var reloaded = new ViewPreferences(paths, NullLogger<ViewPreferences>.Instance);
        Assert.Empty(reloaded.Snapshot());
    }

    [Fact]
    public void 目录移动_前缀范围内的folder键跟随()
    {
        var (_, prefs) = Create();
        prefs.Set("folder:a", new ViewSort("name", "asc"));
        prefs.Set("folder:a/b", new ViewSort("size", "desc"));
        prefs.Set("folder:a/b/c", new ViewSort("star", "asc"));
        prefs.Set("folder:ab", new ViewSort("name", "desc")); // 前缀相似但不同目录,不受影响
        prefs.Set("tag:a", new ViewSort("name", "asc"));      // 非 folder 作用域,不受影响

        prefs.RenamePrefix("a", "x/y");

        var all = prefs.Snapshot();
        Assert.False(all.ContainsKey("folder:a"));
        Assert.Equal(new ViewSort("size", "desc"), all["folder:x/y/b"]);
        Assert.Equal(new ViewSort("star", "asc"), all["folder:x/y/b/c"]);
        Assert.Equal(new ViewSort("name", "asc"), all["folder:x/y"]);
        Assert.Equal(new ViewSort("name", "desc"), all["folder:ab"]);
        Assert.Equal(new ViewSort("name", "asc"), all["tag:a"]);
    }

    [Fact]
    public void 目录删除_前缀范围内的folder键清除()
    {
        var (_, prefs) = Create();
        prefs.Set("folder:a/b", new ViewSort("name", "asc"));
        prefs.Set("folder:a", new ViewSort("size", "desc"));

        prefs.DeletePrefix("a");

        Assert.Empty(prefs.Snapshot());
    }

    [Theory]
    [InlineData("folder:设计素材/海报", true)]
    [InlineData("folder:", true)]            // 库根
    [InlineData("folder:a/./b", false)]      // 越界
    [InlineData("folder:.hawk/x", false)]    // .hawk 内部
    [InlineData("category:品牌", true)]
    [InlineData("category:a/b", false)]      // 分类名不含斜杠
    [InlineData("tag:logo", true)]
    [InlineData("tag:  ", false)]
    [InlineData("item:a", false)]
    public void scope校验(string raw, bool expected)
    {
        Assert.Equal(expected, ViewPreferences.TryParseScope(raw, out _));
    }

    [Theory]
    [InlineData("name", "asc", true)]
    [InlineData("star", "DESC", true)]       // 大小写归一
    [InlineData("modification_time", "desc", true)]
    [InlineData("size", "up", false)]
    [InlineData("created_at", "asc", false)]
    public void 排序值校验(string orderBy, string order, bool expected)
    {
        Assert.Equal(expected, ViewPreferences.TryNormalizeSort(orderBy, order, out _));
    }

    [Fact]
    public void 归一化_大小写与空白()
    {
        Assert.True(ViewPreferences.TryParseScope("tag:  logo  ", out var scope));
        Assert.Equal("tag:logo", scope);

        Assert.True(ViewPreferences.TryNormalizeSort(" Name ", " ASC ", out var sort));
        Assert.Equal(new ViewSort("name", "asc"), sort);
    }

    [Fact]
    public void 损坏的文件按空表处理()
    {
        var (paths, _) = Create();
        File.WriteAllText(paths.ViewFile, "not [ valid toml");

        var prefs = new ViewPreferences(paths, NullLogger<ViewPreferences>.Instance);
        Assert.Empty(prefs.Snapshot());
    }

    [Fact]
    public void 无效条目被跳过_合法条目保留()
    {
        var (paths, prefs) = Create();
        prefs.Set("tag:ok", new ViewSort("name", "asc"));
        File.AppendAllText(paths.ViewFile, "[\"tag:bad entry\"]\norder_by = \"unknown\"\norder = \"asc\"\n");

        var reloaded = new ViewPreferences(paths, NullLogger<ViewPreferences>.Instance);
        var all = reloaded.Snapshot();
        Assert.Single(all);
        Assert.Equal(new ViewSort("name", "asc"), all["tag:ok"]);
    }
}

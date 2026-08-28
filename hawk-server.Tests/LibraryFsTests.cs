using Hawk.Server.Core;

namespace Hawk.Server.Tests;

public class LibraryFsTests
{
    private readonly TempDir _dir = new();

    [Theory]
    [InlineData("图标", true)]
    [InlineData("a b", true)]
    [InlineData("", false)]
    [InlineData("a/b", false)]
    [InlineData("a\\b", false)]
    [InlineData(".", false)]
    [InlineData("..", false)]
    [InlineData(".hawk", false)]
    public void IsValidName_规则(string? name, bool expected)
    {
        Assert.Equal(expected, LibraryFs.IsValidName(name));
    }

    [Fact]
    public void FindFreeTrashPath_无冲突时保留原结构()
    {
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();

        var abs = LibraryFs.FindFreeTrashPath(paths, "posters/cat.jpg", isDirectory: false);
        Assert.Equal(Path.Combine(paths.TrashDir, "posters", "cat.jpg"), abs);
    }

    [Fact]
    public void FindFreeTrashPath_冲突时在扩展名前追加序号()
    {
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();
        _dir.WriteText(".hawk/trash/posters/cat.jpg", "occupied");
        _dir.WriteText(".hawk/trash/posters/cat (1).jpg", "occupied");

        var abs = LibraryFs.FindFreeTrashPath(paths, "posters/cat.jpg", isDirectory: false);
        Assert.Equal(Path.Combine(paths.TrashDir, "posters", "cat (2).jpg"), abs);
    }

    [Fact]
    public void FindFreeTrashPath_目录冲突在末尾追加序号()
    {
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();
        _dir.Mkdir(".hawk/trash/posters");

        var abs = LibraryFs.FindFreeTrashPath(paths, "posters", isDirectory: true);
        Assert.Equal(Path.Combine(paths.TrashDir, "posters (1)"), abs);
    }
}

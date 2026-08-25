using Hawk.Server.Core;

namespace Hawk.Server.Tests;

public class LibraryPathsTests
{
    private readonly TempDir _dir = new();
    private LibraryPaths Paths => new(_dir.Root);

    [Fact]
    public void ToRelative_根目录返回空串()
    {
        Assert.Equal("", Paths.ToRelative(_dir.Root));
    }

    [Fact]
    public void ToRelative_库内路径转为正斜杠相对路径()
    {
        var abs = Path.Combine(_dir.Root, "posters", "2024", "cat.jpg");
        Assert.Equal("posters/2024/cat.jpg", Paths.ToRelative(abs));
    }

    [Fact]
    public void ToRelative_库外路径返回null()
    {
        Assert.Null(Paths.ToRelative(Path.Combine(_dir.Root, "..", "outside.txt")));
        Assert.Null(Paths.ToRelative("/etc/passwd"));
    }

    [Fact]
    public void ToAbsolute_正常换算()
    {
        Assert.Equal(Path.Combine(_dir.Root, "a", "b.png"), Paths.ToAbsolute("a/b.png"));
    }

    [Theory]
    [InlineData("../escape.txt")]
    [InlineData("a/../../escape.txt")]
    [InlineData("a/./b.png")]
    [InlineData("/absolute/path")]
    [InlineData("")]
    public void ToAbsolute_拒绝越界与非法路径(string rel)
    {
        Assert.Null(Paths.ToAbsolute(rel));
    }

    [Theory]
    [InlineData(".hawk", true)]
    [InlineData(".hawk/metadata/abc.toml", true)]
    [InlineData(".hawk/thumbnails/256/ab/x.webp", true)]
    [InlineData(".hawk/trash/a.jpg", false)] // 回收站参与索引
    [InlineData(".hawk/trash", true)]        // 回收站目录本身不是文件位置
    [InlineData("posters/a.jpg", false)]
    public void IsInternal_规则(string rel, bool expected)
    {
        Assert.Equal(expected, LibraryPaths.IsInternal(rel));
    }

    [Fact]
    public void Trash路径换算_往返一致()
    {
        const string lib = "posters/2024/cat.jpg";
        var trash = LibraryPaths.LibraryToTrashPath(lib);
        Assert.Equal(".hawk/trash/posters/2024/cat.jpg", trash);
        Assert.True(LibraryPaths.IsInTrash(trash));
        Assert.Equal(lib, LibraryPaths.TrashToLibraryPath(trash));
    }

    [Theory]
    [InlineData("a/b.png", true)]
    [InlineData(".hawk/trash/a.png", true)] // 回收站位置是合法参数（restore 场景）
    [InlineData(".hawk/metadata/x.toml", false)]
    [InlineData("a/../b", false)]
    [InlineData("a\\b", false)]
    [InlineData("/a", false)]
    [InlineData("", false)]
    public void IsValidLibraryPath_规则(string rel, bool expected)
    {
        Assert.Equal(expected, LibraryPaths.IsValidLibraryPath(rel));
    }

    [Theory]
    [InlineData("a/b/cat.jpg", "a/b")]
    [InlineData("cat.jpg", "")]
    public void DirOf_取父目录(string rel, string expected)
    {
        Assert.Equal(expected, LibraryPaths.DirOf(rel));
    }

    [Theory]
    [InlineData("a/cat.jpg", "cat", "jpg")]
    [InlineData("a/archive.tar.gz", "archive.tar", "gz")]
    [InlineData("a/.hidden", ".hidden", "")]
    [InlineData("a/noext", "noext", "")]
    [InlineData("a/PHOTO.JPEG", "PHOTO", "jpeg")]
    public void NameOf与ExtOf(string rel, string name, string ext)
    {
        Assert.Equal(name, LibraryPaths.NameOf(rel));
        Assert.Equal(ext, LibraryPaths.ExtOf(rel));
    }
}

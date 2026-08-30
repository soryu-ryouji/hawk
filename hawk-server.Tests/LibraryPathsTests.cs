using Hawk.Server.Core;

namespace Hawk.Server.Tests;

public class LibraryPathsTests
{
    private readonly TempDir _dir = new();
    private LibraryPaths Paths => new(_dir.Root, _dir.CacheRoot);

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

    [Fact]
    public void 派生缓存默认位于库外系统缓存目录()
    {
        using var dir = new TempDir();
        var paths = new LibraryPaths(dir.Root);

        // 在库外（不会被库扫描/同步进 iCloud、Dropbox）
        Assert.DoesNotContain(paths.ThumbnailsDir, dir.Root);
        Assert.DoesNotContain(paths.IndexDbFile, dir.Root);
        // 按库区分：不同库根 → 不同缓存子目录
        var other = new LibraryPaths(dir.Root + "-other");
        Assert.NotEqual(paths.ThumbnailsDir, other.ThumbnailsDir);
        // 库数据仍在 .hawk/ 内
        Assert.StartsWith(Path.Combine(dir.Root, ".hawk"), paths.MetadataDir);
        Assert.StartsWith(Path.Combine(dir.Root, ".hawk"), paths.TrashDir);
    }

    [Fact]
    public void EnsureLayout_缓存目录建在覆盖位置且hawk内不再有缓存目录()
    {
        using var dir = new TempDir();
        var paths = new LibraryPaths(dir.Root, dir.CacheRoot);
        paths.EnsureLayout();

        Assert.Equal(Path.Combine(dir.CacheRoot, "thumbnails"), paths.ThumbnailsDir);
        Assert.Equal(Path.Combine(dir.CacheRoot, "index.db"), paths.IndexDbFile);
        Assert.True(Directory.Exists(paths.ThumbnailsDir));
        Assert.False(Directory.Exists(Path.Combine(dir.Root, ".hawk", "thumbnails")));
    }
    [Fact]
    public void 缓存目录名_库文件夹名加哈希前缀且同名库可区分()
    {
        using var dir = new TempDir();
        var root = Path.Combine(dir.Root, "Design Refs");
        Directory.CreateDirectory(root);
        var paths = new LibraryPaths(root);

        var name = Path.GetFileName(paths.CacheDir);
        var underscore = name.IndexOf('_');
        Assert.True(underscore > 0); // 库名前缀
        Assert.Equal(16, name.Length - underscore - 1); // 16 位哈希
        Assert.True(name[(underscore + 1)..].All(c => c is >= '0' and <= '9' or >= 'a' and <= 'f'));
        Assert.StartsWith("Design Refs_", name);

        // 同名库（不同路径）→ 哈希部分不同，目录可区分
        var otherRoot = Path.Combine(dir.Root, "nested", "Design Refs");
        Directory.CreateDirectory(otherRoot);
        var other = new LibraryPaths(otherRoot);
        Assert.NotEqual(paths.CacheDir, other.CacheDir);
    }

    [Fact]
    public void EnsureLayout_旧版纯哈希缓存目录自动改名迁移()
    {
        using var dir = new TempDir();
        // 与实现相同的算法算出旧目录名，预置标记文件
        var hash = System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(dir.Root));
        var legacyName = Convert.ToHexString(hash)[..16].ToLowerInvariant();
        var cacheParent = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "hawk", "cache");
        var legacy = Path.Combine(cacheParent, legacyName);
        var migrated = Path.Combine(cacheParent, Path.GetFileName(new LibraryPaths(dir.Root).CacheDir));
        var paths = new LibraryPaths(dir.Root);
        try
        {
            Directory.CreateDirectory(legacy);
            File.WriteAllText(Path.Combine(legacy, "marker.txt"), "keep");

            paths.EnsureLayout();

            Assert.False(Directory.Exists(legacy));
            Assert.Equal("keep", File.ReadAllText(Path.Combine(paths.CacheDir, "marker.txt")));
        }
        finally
        {
            if (Directory.Exists(legacy)) Directory.Delete(legacy, recursive: true);
            if (Directory.Exists(migrated)) Directory.Delete(migrated, recursive: true);
        }
    }

}

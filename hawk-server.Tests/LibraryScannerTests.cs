using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;

namespace Hawk.Server.Tests;

public class LibraryScannerTests
{
    private readonly TempDir _dir = new();

    [Fact]
    public void 遍历跳过hawk内部但包含回收站()
    {
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();
        _dir.WriteFile("a.png", TempDir.TinyPng);
        _dir.WriteFile("sub/b.png", TempDir.TinyPng);
        _dir.WriteText(".hawk/metadata/" + new string('a', 64) + ".toml", "star = 1");
        _dir.WriteFile(".hawk/trash/old.png", TempDir.TinyPng);
        _dir.WriteText(".hawk/config.toml", "");

        var config = new LibraryConfig(paths, NullLogger<LibraryConfig>.Instance);
        var scanner = new LibraryScanner(paths, config);
        var rels = scanner.WalkLibrary().Select(p => paths.ToRelative(p)!).Order().ToArray();

        Assert.Equal([".hawk/trash/old.png", "a.png", "sub/b.png"], rels);
    }

    [Fact]
    public void 遍历应用ignore规则剪枝()
    {
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();
        File.WriteAllText(paths.ConfigFile, """ignore = ["node_modules", "*.tmp"]""");
        _dir.WriteFile("keep.png", TempDir.TinyPng);
        _dir.WriteFile("node_modules/skip.png", TempDir.TinyPng);
        _dir.WriteFile("a/b.tmp", [1]);

        var config = new LibraryConfig(paths, NullLogger<LibraryConfig>.Instance);
        var scanner = new LibraryScanner(paths, config);
        var rels = scanner.WalkLibrary().Select(p => paths.ToRelative(p)!).ToArray();

        Assert.Equal(["keep.png"], rels);
    }
}

using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;

namespace Hawk.Server.Tests;

public class LibraryConfigTests
{
    private readonly TempDir _dir = new();

    private LibraryConfig Load(string? configToml)
    {
        var paths = new LibraryPaths(_dir.Root);
        paths.EnsureLayout();
        if (configToml is not null)
        {
            File.WriteAllText(paths.ConfigFile, configToml);
        }

        return new LibraryConfig(paths, NullLogger<LibraryConfig>.Instance);
    }

    [Fact]
    public void 无配置文件时使用默认值()
    {
        var config = Load(null);
        Assert.Null(config.Current.Name);
        Assert.Empty(config.Current.Ignore);
        Assert.Equal([256, 1024], config.Current.ThumbnailSizes);
    }

    [Fact]
    public void 解析名称与缩略图尺寸()
    {
        var config = Load("""
            name = "设计素材库"
            thumbnail_sizes = [512]
            """);
        Assert.Equal("设计素材库", config.Current.Name);
        Assert.Equal([512], config.Current.ThumbnailSizes);
    }

    [Fact]
    public void 损坏的配置回退默认值()
    {
        var config = Load("这不是合法 TOML = [");
        Assert.Null(config.Current.Name);
        Assert.Equal(LibraryConfig.DefaultThumbnailSizes, config.Current.ThumbnailSizes);
    }

    [Theory]
    [InlineData("node_modules", true)]              // 根级目录
    [InlineData("a/node_modules", true)]            // 任意深度同名目录
    [InlineData("node_modules/lib/x.js", true)]     // 目录内容一并排除（监听事件以文件路径到达）
    [InlineData("a/node_modules/lib/x.js", true)]
    [InlineData("build/out", true)]                 // 含 / 的路径模式
    [InlineData("build/out/x.png", true)]           // 路径模式的内容
    [InlineData("build/out.png", false)]            // 模式不等同于前缀：不匹配同名前缀文件
    [InlineData("build/output/deep/x.png", false)]  // 不误伤前缀相近目录
    [InlineData("a.tmp", true)]                     // glob 模式
    [InlineData("x/y/b.tmp", true)]
    [InlineData("node_modules2/x.js", false)]       // 不误伤前缀相近项
    [InlineData("src/build/x.png", false)]          // 含 / 的模式不任意深度
    [InlineData("normal.png", false)]
    public void Ignore匹配规则(string relPath, bool expected)
    {
        var config = Load("""
            ignore = ["node_modules", "*.tmp", "build/out"]
            """);
        Assert.Equal(expected, config.IsIgnored(relPath));
    }

    [Theory]
    [InlineData("build/out", true)]
    [InlineData("build/out/a.png", true)]
    [InlineData("build/output", false)]           // 前缀相近目录不误伤
    [InlineData("build/output/a.png", false)]
    public void Ignore目录模式不误伤前缀相近目录(string relPath, bool expected)
    {
        var config = Load("""ignore = ["build/out"]""");
        Assert.Equal(expected, config.IsIgnored(relPath));
    }
}

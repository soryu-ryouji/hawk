using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;
using SixLabors.ImageSharp;
using SixLabors.ImageSharp.PixelFormats;

namespace Hawk.Server.Tests;

public class ColorServiceTests
{
    private readonly TempDir _dir = new();

    private ColorService NewService() => new(NullLogger<ColorService>.Instance);

    [Fact]
    public void 提炼结果随元数据TOML持久化并跨实例恢复()
    {
        // 调色板是内容的纯函数:写入素材元数据 TOML(参与网盘同步),一台计算全平台复用——
        // 新实例(其他设备/平台)从 TOML 解析即得,无需重新提炼
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();
        var hash = new string('a', 64);
        var store = new MetadataStore(paths, new IndexDb(paths, NullLogger<IndexDb>.Instance), NullLogger<MetadataStore>.Instance);
        store.Save(hash, new ItemMetadata
        {
            Width = 640,
            Height = 480,
            Palette = [new PaletteEntry("#344441", 3.1f), new PaletteEntry("#ff0000", 96.9f)],
        });

        // 模拟另一台设备/平台:全新 MetadataStore 从 TOML 解析
        var restored = new MetadataStore(paths, new IndexDb(paths, NullLogger<IndexDb>.Instance), NullLogger<MetadataStore>.Instance);
        Assert.True(restored.TryGet(hash, out var meta));
        Assert.Equal(640, meta.Width);
        Assert.Equal(480, meta.Height);
        Assert.Equal(2, meta.Palette!.Count);
        Assert.Equal("#344441", meta.Palette[0].Color);
        Assert.Equal(3.1f, meta.Palette[0].Percentage);

        // 索引同步路径:PaletteEntry → PaletteColor(Lab 由 RGB 纯函数重算)
        var item = new Item { Id = hash };
        item.SyncFrom(meta);
        Assert.Equal(640, item.Width);
        Assert.Equal(2, item.Palette.Length);
        Assert.Equal("#344441", HexOf(item.Palette[0]));
        Assert.Equal(3.1f, item.Palette[0].Percentage);
        Assert.Equal(PaletteColor.FromRgb(0x34, 0x44, 0x41, 3.1f).Lab, item.Palette[0].Lab);
    }

    private static string SavePng(string absPath, Image<Rgba32> image)
    {
        image.SaveAsPng(absPath);
        return absPath;
    }

    private static Image<Rgba32> Solid(int w, int h, Rgba32 color) => new(w, h, color);

    private static string HexOf(PaletteColor p) => ColorMath.ToHex(p.R, p.G, p.B);

    [Fact]
    public void 提炼_纯色图为单一颜色()
    {
        using var image = Solid(64, 64, new Rgba32(255, 0, 0));
        var palette = ColorService.Extract(image);

        var color = Assert.Single(palette);
        Assert.Equal("#ff0000", HexOf(color));
        Assert.Equal(100f, color.Percentage);
    }

    [Fact]
    public void 提炼_双色各半()
    {
        using var image = Solid(64, 64, new Rgba32(255, 0, 0));
        for (var y = 0; y < 64; y++)
        {
            for (var x = 32; x < 64; x++)
            {
                image[x, y] = new Rgba32(0, 0, 255);
            }
        }

        var palette = ColorService.Extract(image);

        Assert.Equal(2, palette.Length);
        Assert.Equal(["#0000ff", "#ff0000"], palette.Select(HexOf).OrderBy(c => c).ToArray());
        Assert.All(palette, p => Assert.InRange(p.Percentage, 49f, 51f));
    }

    [Fact]
    public void 提炼_全透明图为空调色板()
    {
        using var image = Solid(64, 64, new Rgba32(255, 0, 0, 0));
        Assert.Empty(ColorService.Extract(image));
    }

    [Fact]
    public void 提炼_忽略半透明以下的像素()
    {
        // 左半不透明红，右半几乎全透明的蓝 → 只统计红色
        using var image = Solid(64, 64, new Rgba32(255, 0, 0));
        for (var y = 0; y < 64; y++)
        {
            for (var x = 32; x < 64; x++)
            {
                image[x, y] = new Rgba32(0, 0, 255, 127);
            }
        }

        var color = Assert.Single(ColorService.Extract(image));
        Assert.Equal("#ff0000", HexOf(color));
        Assert.Equal(100f, color.Percentage);
    }

    [Fact]
    public void 提炼_非图像文件返回Null()
    {
        var file = _dir.WriteText("not-image.txt", "hello");
        Assert.Null(NewService().Extract(file));
    }

    [Fact]
    public void 提炼与缓存_从缩略图文件提炼()
    {
        // 模拟 worker 流程：从落盘的缩略图提炼
        var service = NewService();
        using var image = Solid(256, 256, new Rgba32(0x34, 0x44, 0x41));
        var source = SavePng(Path.Combine(_dir.Root, "thumb.png"), image);

        var palette = service.Extract(source);
        Assert.NotNull(palette);
        var color = Assert.Single(palette);
        Assert.Equal("#344441", HexOf(color));
        Assert.Equal(100f, color.Percentage);
    }
}

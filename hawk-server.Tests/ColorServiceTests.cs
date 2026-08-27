using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;
using SixLabors.ImageSharp;
using SixLabors.ImageSharp.PixelFormats;

namespace Hawk.Server.Tests;

public class ColorServiceTests
{
    private readonly TempDir _dir = new();

    private ColorService NewService() => new(new LibraryPaths(_dir.Root), NullLogger<ColorService>.Instance);

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
    public void 缓存_写入后可读回且一致()
    {
        var service = NewService();
        var hash = new string('a', 64);
        var palette = new[]
        {
            PaletteColor.FromRgb(0x34, 0x44, 0x41, 3.1f),
            PaletteColor.FromRgb(0xFF, 0x00, 0x00, 96.9f),
        };

        service.Save(hash, palette);
        Assert.True(service.Exists(hash));

        var loaded = service.Load(hash);
        Assert.NotNull(loaded);
        Assert.Equal(2, loaded.Length);
        Assert.Equal("#344441", HexOf(loaded[0]));
        Assert.Equal(3.1f, loaded[0].Percentage);
        Assert.Equal("#ff0000", HexOf(loaded[1]));
        Assert.Equal(96.9f, loaded[1].Percentage);
        // 读回时重新预算 Lab，与提取结果一致
        Assert.Equal(palette[0].Lab, loaded[0].Lab);
    }

    [Fact]
    public void 缓存_版本不符视为缺失()
    {
        var service = NewService();
        var hash = new string('b', 64);
        var file = service.GetPath(hash);
        Directory.CreateDirectory(Path.GetDirectoryName(file)!);
        File.WriteAllText(file, """{"v": 999, "palette": [{"color": "#ff0000", "percentage": 100.0}]}""");

        Assert.True(service.Exists(hash));
        Assert.Null(service.Load(hash));
    }

    [Fact]
    public void 缓存_损坏文件视为缺失()
    {
        var service = NewService();
        var hash = new string('c', 64);
        var file = service.GetPath(hash);
        Directory.CreateDirectory(Path.GetDirectoryName(file)!);
        File.WriteAllText(file, "not json");

        Assert.Null(service.Load(hash));
    }

    [Fact]
    public void 提炼与缓存_从缩略图文件提炼()
    {
        // 模拟 worker 流程：从落盘的缩略图提炼 → 写缓存 → 读回
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

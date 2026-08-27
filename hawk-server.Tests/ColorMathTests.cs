using Hawk.Server.Core;

namespace Hawk.Server.Tests;

public class ColorMathTests
{
    [Theory]
    [InlineData("#344441", 0x34, 0x44, 0x41)]
    [InlineData("344441", 0x34, 0x44, 0x41)]
    [InlineData("#FF0000", 0xFF, 0x00, 0x00)]
    [InlineData(" #abcdef ", 0xAB, 0xCD, 0xEF)]
    public void ParseHex_合法输入(string text, byte r, byte g, byte b)
    {
        Assert.Equal((r, g, b), ColorMath.ParseHex(text));
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("#12345")]
    [InlineData("#1234567")]
    [InlineData("gggggg")]
    [InlineData("#")]
    public void ParseHex_非法输入返回Null(string? text)
    {
        Assert.Null(ColorMath.ParseHex(text));
    }

    [Fact]
    public void ToHex_小写带前缀()
    {
        Assert.Equal("#344441", ColorMath.ToHex(0x34, 0x44, 0x41));
        Assert.Equal("#ff0000", ColorMath.ToHex(0xFF, 0x00, 0x00));
    }

    [Fact]
    public void RgbToLab_白色()
    {
        var lab = ColorMath.RgbToLab(255, 255, 255);
        Assert.InRange(lab.L, 99.9, 100.1);
        Assert.InRange(lab.A, -0.1, 0.1);
        Assert.InRange(lab.B, -0.1, 0.1);
    }

    [Fact]
    public void RgbToLab_黑色()
    {
        var lab = ColorMath.RgbToLab(0, 0, 0);
        Assert.InRange(lab.L, -0.1, 0.1);
        Assert.InRange(lab.A, -0.1, 0.1);
        Assert.InRange(lab.B, -0.1, 0.1);
    }

    [Fact]
    public void RgbToLab_已知向量()
    {
        // #344441（手算：X/Xn≈0.04667, Y≈0.05251, Z/Zn≈0.05294）
        var lab = ColorMath.RgbToLab(0x34, 0x44, 0x41);
        Assert.InRange(lab.L, 26.9, 27.9);
        Assert.InRange(lab.A, -7.8, -6.8);
        Assert.InRange(lab.B, -0.7, 0.3);
    }

    [Fact]
    public void DeltaESquared_相同颜色为零()
    {
        var lab = ColorMath.RgbToLab(0x34, 0x44, 0x41);
        Assert.Equal(0, ColorMath.DeltaESquared(lab, lab));
    }

    [Fact]
    public void DeltaESquared_已知距离()
    {
        var a = new LabColor(50, 0, 0);
        var b = new LabColor(50, 3, 4);
        Assert.Equal(25, ColorMath.DeltaESquared(a, b));
    }
}

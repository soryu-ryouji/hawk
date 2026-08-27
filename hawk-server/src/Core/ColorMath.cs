namespace Hawk.Server.Core;

/// <summary>CIELAB 色彩空间中的一个颜色（感知均匀，用于颜色相似度比较）</summary>
public readonly record struct LabColor(double L, double A, double B);

/// <summary>颜色工具：hex 解析/格式化、sRGB→CIELAB 转换、CIE76 ΔE 距离。纯函数。</summary>
public static class ColorMath
{
    /// <summary>解析 "#344441" / "344441"（大小写不敏感）为 RGB；非法返回 null</summary>
    public static (byte R, byte G, byte B)? ParseHex(string? text)
    {
        if (string.IsNullOrWhiteSpace(text))
        {
            return null;
        }

        var s = text.Trim();
        if (s.StartsWith('#'))
        {
            s = s[1..];
        }

        if (s.Length != 6 || !uint.TryParse(s, System.Globalization.NumberStyles.HexNumber, null, out var v))
        {
            return null;
        }

        return ((byte)(v >> 16), (byte)(v >> 8), (byte)v);
    }

    /// <summary>RGB → "#344441"（小写 # 前缀）</summary>
    public static string ToHex(byte r, byte g, byte b) => $"#{r:x2}{g:x2}{b:x2}";

    /// <summary>sRGB → CIELAB（D65 光源）</summary>
    public static LabColor RgbToLab(byte r, byte g, byte b)
    {
        var rl = PivotRgb(r / 255.0);
        var gl = PivotRgb(g / 255.0);
        var bl = PivotRgb(b / 255.0);

        var x = (rl * 0.4124564 + gl * 0.3575761 + bl * 0.1804375) / 0.95047;
        var y = rl * 0.2126729 + gl * 0.7151522 + bl * 0.0721750;
        var z = (rl * 0.0193339 + gl * 0.1191920 + bl * 0.9503041) / 1.08883;

        var fx = PivotXyz(x);
        var fy = PivotXyz(y);
        var fz = PivotXyz(z);

        return new LabColor(116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz));
    }

    /// <summary>CIE76 ΔE 的平方。与阈值的平方比较，免去逐像素开方</summary>
    public static double DeltaESquared(LabColor a, LabColor b)
    {
        var dl = a.L - b.L;
        var da = a.A - b.A;
        var db = a.B - b.B;
        return dl * dl + da * da + db * db;
    }

    private static double PivotRgb(double c) => c <= 0.04045 ? c / 12.92 : Math.Pow((c + 0.055) / 1.055, 2.4);

    private static double PivotXyz(double t) => t > 0.008856 ? Math.Pow(t, 1.0 / 3.0) : 7.787 * t + 16.0 / 116.0;
}

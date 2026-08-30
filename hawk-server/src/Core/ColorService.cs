using SixLabors.ImageSharp;
using SixLabors.ImageSharp.PixelFormats;
using SixLabors.ImageSharp.Processing;
using SixLabors.ImageSharp.Processing.Processors.Quantization;

namespace Hawk.Server.Core;

/// <summary>
/// 调色板提炼器：从图像提炼代表颜色（降采样 → Wu 量化 → 像素占比统计）。
/// 提炼结果作为「内容的纯函数」直接写入素材元数据 TOML（参与同步，一台计算全平台复用），
/// 本类不再持有缓存——存在性/读写都走 MetadataStore。
/// </summary>
public sealed class ColorService
{
    /// <summary>调色板最大颜色数</summary>
    public const int PaletteSize = 10;

    /// <summary>提炼前降采样到的最大边长（提速并抹掉噪点）</summary>
    public const int AnalysisSize = 64;

    /// <summary>调色板算法版本：提炼算法或参数变更时 +1，TOML 中的旧版本结果视为未提炼（触发重新提炼）</summary>
    public const int PaletteVersion = 1;

    private readonly ILogger<ColorService> _logger;

    public ColorService(ILogger<ColorService> logger)
    {
        _logger = logger;
    }

    /// <summary>
    /// 从图像文件提炼调色板。源文件一般是已有的小尺寸缩略图，解码代价极低。
    /// 解码失败返回 null（下次重试）；图像无有效像素返回空数组（缓存之，不再重试）。
    /// </summary>
    public PaletteColor[]? Extract(string sourceAbs)
    {
        try
        {
            using var image = Image.Load<Rgba32>(sourceAbs);
            return Extract(image);
        }
        catch (Exception ex)
        {
            _logger.LogDebug(ex, "提炼调色板失败: {Path}", sourceAbs);
            return null;
        }
    }

    /// <summary>
    /// 从已解码图像提炼调色板（动图取首帧）：降采样到 AnalysisSize 后 Wu 量化，
    /// 统计各代表色的像素占比（alpha &lt; 128 的像素不参与），按占比降序取前 PaletteSize 个。
    /// </summary>
    public static PaletteColor[] Extract(Image<Rgba32> image)
    {
        using var small = image.Clone(x => x.Resize(new ResizeOptions
        {
            Size = new Size(AnalysisSize, AnalysisSize),
            Mode = ResizeMode.Max,
        }));

        // Wu 量化把像素归并到至多 PaletteSize 个代表色；关闭抖动避免噪点干扰占比统计
        using var quantized = small.Clone(x => x.Quantize(new WuQuantizer(new QuantizerOptions { MaxColors = PaletteSize, Dither = null })));

        var counts = new Dictionary<int, int>();
        var total = 0;
        quantized.ProcessPixelRows(accessor =>
        {
            for (var y = 0; y < accessor.Height; y++)
            {
                foreach (var pixel in accessor.GetRowSpan(y))
                {
                    if (pixel.A < 128)
                    {
                        continue;
                    }

                    var key = (pixel.R << 16) | (pixel.G << 8) | pixel.B;
                    counts[key] = counts.GetValueOrDefault(key) + 1;
                    total++;
                }
            }
        });

        if (total == 0)
        {
            return [];
        }

        return counts
            .OrderByDescending(kv => kv.Value)
            .ThenBy(kv => kv.Key)
            .Take(PaletteSize)
            .Select(kv => PaletteColor.FromRgb(
                (byte)(kv.Key >> 16),
                (byte)(kv.Key >> 8),
                (byte)kv.Key,
                (float)Math.Round(kv.Value * 1000.0 / total) / 10f))
            .ToArray();
    }
}

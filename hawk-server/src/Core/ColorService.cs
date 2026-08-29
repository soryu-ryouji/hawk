using System.Text.Json;
using System.Text.Json.Serialization;
using SixLabors.ImageSharp;
using SixLabors.ImageSharp.PixelFormats;
using SixLabors.ImageSharp.Processing;
using SixLabors.ImageSharp.Processing.Processors.Quantization;

namespace Hawk.Server.Core;

/// <summary>
/// 调色板服务：从图像提炼代表颜色（降采样 → Wu 量化 → 像素占比统计），
/// 缓存于库外缓存目录（LibraryPaths.ColorsDir：&lt;系统缓存&gt;/hawk/cache/&lt;库标识&gt;/colors/&lt;hash&gt;.json）。
/// 与缩略图同属可重建的内容寻址本地缓存；缓存带算法版本号，算法变更后旧缓存自动重建。
/// </summary>
public sealed class ColorService
{
    /// <summary>缓存格式版本：提炼算法或参数变更时 +1，旧版本缓存视为缺失</summary>
    public const int CacheVersion = 1;

    /// <summary>调色板最大颜色数</summary>
    public const int PaletteSize = 10;

    /// <summary>提炼前降采样到的最大边长（提速并抹掉噪点）</summary>
    public const int AnalysisSize = 64;

    private readonly LibraryPaths _paths;
    private readonly ILogger<ColorService> _logger;

    public ColorService(LibraryPaths paths, ILogger<ColorService> logger)
    {
        _paths = paths;
        _logger = logger;
    }

    public string GetPath(string hash) => Path.Combine(_paths.ColorsDir, hash + ".json");

    public bool Exists(string hash) => File.Exists(GetPath(hash));

    /// <summary>读取缓存的调色板；不存在、损坏或版本不符时返回 null（视为缺失，触发重建）</summary>
    public PaletteColor[]? Load(string hash)
    {
        try
        {
            var file = GetPath(hash);
            if (!File.Exists(file))
            {
                return null;
            }

            var cache = JsonSerializer.Deserialize<CacheFile>(File.ReadAllText(file));
            if (cache?.Palette is null || cache.V != CacheVersion)
            {
                return null;
            }

            var palette = new List<PaletteColor>(cache.Palette.Count);
            foreach (var entry in cache.Palette)
            {
                if (ColorMath.ParseHex(entry.Color) is { } rgb)
                {
                    palette.Add(PaletteColor.FromRgb(rgb.R, rgb.G, rgb.B, entry.Percentage));
                }
            }

            return palette.ToArray();
        }
        catch (Exception ex) when (ex is IOException or JsonException or UnauthorizedAccessException)
        {
            return null;
        }
    }

    /// <summary>写入调色板缓存（临时文件 + rename，避免半截文件）</summary>
    public void Save(string hash, PaletteColor[] palette)
    {
        var file = GetPath(hash);
        Directory.CreateDirectory(Path.GetDirectoryName(file)!);
        var cache = new CacheFile
        {
            V = CacheVersion,
            Palette = palette.Select(p => new CacheEntry { Color = ColorMath.ToHex(p.R, p.G, p.B), Percentage = p.Percentage }).ToList(),
        };
        var temp = file + ".tmp";
        File.WriteAllText(temp, JsonSerializer.Serialize(cache));
        File.Move(temp, file, overwrite: true);
    }

    public void Delete(string hash)
    {
        var file = GetPath(hash);
        if (File.Exists(file))
        {
            File.Delete(file);
        }
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

    private sealed class CacheFile
    {
        [JsonPropertyName("v")]
        public int V { get; set; }

        [JsonPropertyName("palette")]
        public List<CacheEntry>? Palette { get; set; }
    }

    private sealed class CacheEntry
    {
        [JsonPropertyName("color")]
        public string Color { get; set; } = "";

        [JsonPropertyName("percentage")]
        public float Percentage { get; set; }
    }
}

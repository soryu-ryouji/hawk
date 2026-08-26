using SixLabors.ImageSharp;
using SixLabors.ImageSharp.Formats.Webp;
using SixLabors.ImageSharp.Processing;

namespace Hawk.Server.Core;

/// <summary>
/// 缩略图服务：ImageSharp 解码 + 缩放，输出 WebP。
/// 存储于 .hawk/thumbnails/&lt;size&gt;/&lt;hash前2位&gt;/&lt;hash&gt;.webp，本地缓存可重建。
/// </summary>
public sealed class ThumbnailService
{
    private readonly LibraryPaths _paths;
    private readonly ILogger<ThumbnailService> _logger;

    public ThumbnailService(LibraryPaths paths, ILogger<ThumbnailService> logger)
    {
        _paths = paths;
        _logger = logger;
    }

    public string GetPath(string hash, int size) =>
        Path.Combine(_paths.ThumbnailsDir, size.ToString(), hash[..2], hash + ".webp");

    public bool Exists(string hash, int size) => File.Exists(GetPath(hash, size));

    /// <summary>读取图像尺寸（只解码头信息）。非图像或解码失败返回 null。</summary>
    public static (int Width, int Height)? Identify(string absPath)
    {
        try
        {
            var info = Image.Identify(absPath);
            return ((int)info.Width, (int)info.Height);
        }
        catch
        {
            return null;
        }
    }

    /// <summary>检测图像格式对应的默认扩展名（小写不含点）。无法识别返回 null。</summary>
    public static string? DetectExtension(string absPath)
    {
        try
        {
            var format = Image.DetectFormat(absPath);
            return format?.FileExtensions.FirstOrDefault()?.ToLowerInvariant();
        }
        catch
        {
            return null;
        }
    }

    /// <summary>检测字节流的图像格式扩展名。无法识别返回 null。</summary>
    public static string? DetectExtension(ReadOnlyMemory<byte> data)
    {
        try
        {
            var format = Image.DetectFormat(data.Span);
            return format?.FileExtensions.FirstOrDefault()?.ToLowerInvariant();
        }
        catch
        {
            return null;
        }
    }

    /// <summary>
    /// 为指定内容生成全部配置尺寸的缩略图；已存在的跳过（force 时强制重建）。
    /// 源文件不是图像或已消失时静默跳过——缩略图是尽力而为的缓存。返回是否实际生成了文件。
    /// </summary>
    public async Task<bool> GenerateAsync(string hash, string sourceAbs, IEnumerable<int> sizes, bool force = false, CancellationToken ct = default)
    {
        if (!File.Exists(sourceAbs))
        {
            return false;
        }

        var pending = sizes.Where(s => force || !Exists(hash, s)).ToArray();
        if (pending.Length == 0)
        {
            return false;
        }

        try
        {
            using var image = await Image.LoadAsync(sourceAbs, ct);
            foreach (var size in pending)
            {
                ct.ThrowIfCancellationRequested();
                using var clone = image.Clone(x => x.Resize(new ResizeOptions
                {
                    // Max：等比缩放到边长内，不放大小图
                    Size = new Size(size, size),
                    Mode = ResizeMode.Max,
                }));

                var target = GetPath(hash, size);
                Directory.CreateDirectory(Path.GetDirectoryName(target)!);
                await clone.SaveAsWebpAsync(target, new WebpEncoder { Quality = 80 }, ct);
            }

            return true;
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch (Exception ex)
        {
            _logger.LogDebug(ex, "生成缩略图失败: {Path}", sourceAbs);
            return false;
        }
    }

    /// <summary>删除某内容的全部缩略图</summary>
    public void Delete(string hash, IEnumerable<int> sizes)
    {
        foreach (var size in sizes)
        {
            var file = GetPath(hash, size);
            if (File.Exists(file))
            {
                File.Delete(file);
            }
        }
    }
}

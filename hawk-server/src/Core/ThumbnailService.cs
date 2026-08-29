using SixLabors.ImageSharp;
using SixLabors.ImageSharp.Formats.Webp;
using SixLabors.ImageSharp.Processing;

namespace Hawk.Server.Core;

/// <summary>
/// 缩略图服务：ImageSharp 解码 + 缩放，输出 WebP。
/// 存储于库外缓存目录（LibraryPaths.ThumbnailsDir：&lt;系统缓存&gt;/hawk/cache/&lt;库标识&gt;/thumbnails/&lt;size&gt;/&lt;hash&gt;.webp），本地缓存可重建。
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
        Path.Combine(_paths.ThumbnailsDir, size.ToString(), hash + ".webp");

    public bool Exists(string hash, int size) => File.Exists(GetPath(hash, size));

    /// <summary>读取图像尺寸(只解码头信息)。非图像或解码失败返回 null。
    /// 共享读打开(FileShare.ReadWrite|Delete):后台调用方(扫描/缩略图)不阻塞文件的移动与删除。</summary>
    public static (int Width, int Height)? Identify(string absPath)
    {
        try
        {
            using var stream = OpenShared(absPath);
            var info = Image.Identify(stream);
            return info is null ? null : ((int)info.Width, (int)info.Height);
        }
        catch
        {
            return null;
        }
    }

    /// <summary>检测图像格式对应的默认扩展名(小写不含点)。无法识别返回 null。</summary>
    public static string? DetectExtension(string absPath)
    {
        try
        {
            using var stream = OpenShared(absPath);
            var format = Image.DetectFormat(stream);
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
            using var stream = OpenShared(sourceAbs);
            using var image = await Image.LoadAsync(stream, ct);
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

    /// <summary>共享读打开:缩略图生成可能跨秒持有句柄,允许并发读/移动/删除,不阻塞文件的移动与删除(Windows 句柄语义)</summary>
    private static FileStream OpenShared(string absPath) =>
        new(absPath, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete, 1 << 16, FileOptions.SequentialScan);

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

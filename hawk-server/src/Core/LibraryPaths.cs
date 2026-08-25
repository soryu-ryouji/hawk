namespace Hawk.Server.Core;

/// <summary>
/// 素材库路径工具：负责 .hawk/ 内部路径、库内相对路径换算与安全校验。
/// 索引与 API 层统一使用正斜杠相对路径（相对素材库根目录），如 "posters/2024/cat.jpg"。
/// </summary>
public sealed class LibraryPaths
{
    public const string HawkDirName = ".hawk";
    public const string TrashDirName = "trash";

    /// <summary>回收站位置的路径前缀（相对库根目录）</summary>
    public const string TrashPrefix = HawkDirName + "/" + TrashDirName + "/";

    public string Root { get; }
    public string HawkDir { get; }
    public string MetadataDir { get; }
    public string ThumbnailsDir { get; }
    public string TrashDir { get; }
    public string ConfigFile { get; }

    public LibraryPaths(string root)
    {
        Root = Path.GetFullPath(root);
        HawkDir = Path.Combine(Root, HawkDirName);
        MetadataDir = Path.Combine(HawkDir, "metadata");
        ThumbnailsDir = Path.Combine(HawkDir, "thumbnails");
        TrashDir = Path.Combine(HawkDir, TrashDirName);
        ConfigFile = Path.Combine(HawkDir, "config.toml");
    }

    /// <summary>创建 .hawk/ 目录结构，并生成排除缓存目录的 .gitignore</summary>
    public void EnsureLayout()
    {
        Directory.CreateDirectory(MetadataDir);
        Directory.CreateDirectory(ThumbnailsDir);
        Directory.CreateDirectory(TrashDir);

        var gitignore = Path.Combine(HawkDir, ".gitignore");
        if (!File.Exists(gitignore))
        {
            File.WriteAllText(gitignore, "thumbnails/\ntrash/\n");
        }
    }

    /// <summary>绝对路径 → 库内相对路径（正斜杠）。库根目录返回 ""，不在库内时返回 null。</summary>
    public string? ToRelative(string absPath)
    {
        var rel = Path.GetRelativePath(Root, absPath);
        if (rel == ".")
        {
            return "";
        }

        if (rel.StartsWith("..") || Path.IsPathRooted(rel))
        {
            return null;
        }

        return rel.Replace(Path.DirectorySeparatorChar, '/');
    }

    /// <summary>库内相对路径 → 绝对路径。含越界成分（..、绝对路径）时返回 null。</summary>
    public string? ToAbsolute(string relPath)
    {
        if (string.IsNullOrWhiteSpace(relPath) || Path.IsPathRooted(relPath))
        {
            return null;
        }

        var segments = relPath.Split('/', StringSplitOptions.RemoveEmptyEntries);
        if (segments.Length == 0 || segments.Any(s => s is "." or ".."))
        {
            return null;
        }

        var abs = Path.GetFullPath(Path.Combine([Root, .. segments]));
        return abs.StartsWith(Root, StringComparison.Ordinal) ? abs : null;
    }

    /// <summary>是否属于 .hawk/ 内部（回收站除外，回收站参与索引）</summary>
    public static bool IsInternal(string relPath)
    {
        if (relPath.StartsWith(TrashPrefix, StringComparison.Ordinal))
        {
            return false;
        }

        return relPath == HawkDirName || relPath.StartsWith(HawkDirName + "/", StringComparison.Ordinal);
    }

    public static bool IsInTrash(string relPath) => relPath.StartsWith(TrashPrefix, StringComparison.Ordinal);

    /// <summary>回收站位置 → 原库内路径（去掉 .hawk/trash/ 前缀）</summary>
    public static string TrashToLibraryPath(string relPath) => relPath[TrashPrefix.Length..];

    /// <summary>库内路径 → 回收站位置</summary>
    public static string LibraryToTrashPath(string relPath) => TrashPrefix + relPath;

    /// <summary>校验 API 传入的库内相对路径：非空、不指向 .hawk 内部、不含越界成分</summary>
    public static bool IsValidLibraryPath(string? relPath)
    {
        if (string.IsNullOrWhiteSpace(relPath) || relPath.StartsWith('/') || relPath.Contains('\\'))
        {
            return false;
        }

        var segments = relPath.Split('/', StringSplitOptions.RemoveEmptyEntries);
        if (segments.Any(s => s is "." or ".."))
        {
            return false;
        }

        return !IsInternal(relPath);
    }

    /// <summary>取相对路径的父目录部分，根目录返回 ""。</summary>
    public static string DirOf(string relPath)
    {
        var idx = relPath.LastIndexOf('/');
        return idx < 0 ? "" : relPath[..idx];
    }

    /// <summary>文件名（不含扩展名）</summary>
    public static string NameOf(string relPath)
    {
        var fileName = relPath[(relPath.LastIndexOf('/') + 1)..];
        var dot = fileName.LastIndexOf('.');
        return dot <= 0 ? fileName : fileName[..dot];
    }

    /// <summary>扩展名，小写，不含点</summary>
    public static string ExtOf(string relPath)
    {
        var fileName = relPath[(relPath.LastIndexOf('/') + 1)..];
        var dot = fileName.LastIndexOf('.');
        return dot <= 0 ? "" : fileName[(dot + 1)..].ToLowerInvariant();
    }

    /// <summary>Unix 毫秒时间戳</summary>
    public static long ToUnixMs(DateTime utc) => new DateTimeOffset(utc).ToUnixTimeMilliseconds();
}

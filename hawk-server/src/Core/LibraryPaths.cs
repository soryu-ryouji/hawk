namespace Hawk.Server.Core;

/// <summary>
/// 素材库路径工具：负责 .hawk/ 内部路径、库内相对路径换算与安全校验。
/// 索引与 API 层统一使用正斜杠相对路径（相对素材库根目录），如 "posters/2024/cat.jpg"。
/// 缩略图是内容寻址的派生缓存，位于库外系统缓存目录（见 ThumbnailsDir），
/// 避免库在 iCloud/Dropbox 等同步盘时 .hawk/ 膨胀拖累同步；调色板缓存已并入 index.db。
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
    /// <summary>该库派生缓存目录（thumbnails/index.db 的父级），库外系统缓存目录或测试覆盖</summary>
    public string CacheDir { get; }

    /// <summary>cacheDir 参数覆盖（测试用）时缓存命名/迁移逻辑不生效</summary>
    private readonly bool _cacheDirOverridden;

    private static readonly string DefaultCacheParent = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "hawk", "cache");
    /// <summary>元数据 SQLite 派生缓存文件（不参与同步，可删除重建）</summary>
    public string IndexDbFile => Path.Combine(CacheDir, "index.db");
    public string ThumbnailsDir { get; }
    public string TrashDir { get; }
    public string ConfigFile { get; }
    public string CategoriesFile { get; }
    public string TagsFile { get; }
    /// <summary>视图偏好（排序记忆等，参与同步）；损坏按空表处理</summary>
    public string ViewFile { get; }

    /// <param name="cacheDir">该库派生缓存目录（thumbnails/index.db 的父级）的完整路径覆盖；仅供测试指向临时目录，null 时用库外系统缓存目录</param>
    public LibraryPaths(string root, string? cacheDir = null)
    {
        Root = Path.GetFullPath(root);
        HawkDir = Path.Combine(Root, HawkDirName);
        MetadataDir = Path.Combine(HawkDir, "metadata");
        // 缓存按库分目录，多库互不干扰；LocalApplicationData：Windows 为 %LOCALAPPDATA%（iCloud 不同步）。
        // 目录名 = <库文件夹名>_<路径哈希16位>：同名库靠哈希区分，用户可直观识别缓存归属
        _cacheDirOverridden = cacheDir is not null;
        CacheDir = cacheDir ?? Path.Combine(DefaultCacheParent, CacheDirName(Root));
        ThumbnailsDir = Path.Combine(CacheDir, "thumbnails");
        TrashDir = Path.Combine(HawkDir, TrashDirName);
        ConfigFile = Path.Combine(HawkDir, "config.toml");
        CategoriesFile = Path.Combine(HawkDir, "categories.toml");
        TagsFile = Path.Combine(HawkDir, "tags.toml");
        ViewFile = Path.Combine(HawkDir, "view.toml");
    }

    /// <summary>一次性迁移：旧版纯哈希缓存目录改名带上库名前缀（rename 保留全部派生缓存，免于缩略图重建）</summary>
    private void MigrateLegacyCacheDir()
    {
        if (_cacheDirOverridden)
        {
            return;
        }

        var legacy = Path.Combine(DefaultCacheParent, LibraryKey(Root));
        if (Directory.Exists(legacy) && !Directory.Exists(CacheDir))
        {
            Directory.Move(legacy, CacheDir);
        }
    }

    /// <summary>缓存子目录名：库文件夹名_路径哈希前16位（小写十六进制）</summary>
    private static string CacheDirName(string root) => $"{LibraryLabel(root)}_{LibraryKey(root)}";

    /// <summary>库文件夹名（缓存目录的可识别前缀）；非法字符清洗、末尾点/空格去除、截断 32 字符，空名兜底 library</summary>
    private static string LibraryLabel(string root)
    {
        var trimmed = root.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
        var name = new DirectoryInfo(trimmed).Name;
        if (string.IsNullOrWhiteSpace(name))
        {
            return "library";
        }

        var invalid = Path.GetInvalidFileNameChars();
        var label = new string(name.Trim().Select(c => invalid.Contains(c) ? '_' : c).ToArray())
            .TrimEnd('.', ' ');
        if (label.Length == 0)
        {
            return "library";
        }

        return label.Length <= 32 ? label : label[..32];
    }

    /// <summary>库标识：根路径的 SHA-256 前 16 位（小写十六进制），保证多库/同名库缓存目录唯一</summary>
    private static string LibraryKey(string root)
    {
        var hash = System.Security.Cryptography.SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(root));
        return Convert.ToHexString(hash)[..16].ToLowerInvariant();
    }

    /// <summary>创建 .hawk/ 目录结构，并生成排除 trash 的 .gitignore（缺失的排除项会补上）</summary>
    public void EnsureLayout()
    {
        MigrateLegacyCacheDir();
        Directory.CreateDirectory(MetadataDir);
        Directory.CreateDirectory(ThumbnailsDir);
        Directory.CreateDirectory(TrashDir);

        var gitignore = Path.Combine(HawkDir, ".gitignore");
        var required = new[] { "trash/" };
        var existing = File.Exists(gitignore) ? File.ReadAllLines(gitignore) : [];
        var missing = required.Where(r => !existing.Contains(r)).ToArray();
        if (missing.Length > 0)
        {
            File.AppendAllLines(gitignore, missing);
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

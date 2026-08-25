namespace Hawk.Server.Core;

/// <summary>元数据中的一条文件位置记录（对应 TOML 的 [[paths]] 表）</summary>
public sealed class PathEntry
{
    public required string Path { get; set; }
    public long Size { get; set; }
    public long ModificationTime { get; set; }
}

/// <summary>
/// 素材参数元数据，对应 .hawk/metadata/&lt;hash&gt;.toml。
/// 尺寸、扩展名等派生信息不写入元数据，索引时从文件读取。
/// </summary>
public sealed class ItemMetadata
{
    public List<PathEntry> Paths { get; } = new();
    public string? Url { get; set; }
    public List<string> Tags { get; set; } = new();
    public int Star { get; set; }
    public string? Annotation { get; set; }

    public PathEntry? FindPath(string path) => Paths.FirstOrDefault(p => p.Path == path);
}

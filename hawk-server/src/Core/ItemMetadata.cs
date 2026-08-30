namespace Hawk.Server.Core;

/// <summary>元数据中的一条文件位置记录（对应 TOML 的 [[paths]] 表）</summary>
public sealed class PathEntry
{
    public required string Path { get; set; }
    public long Size { get; set; }
    public long ModificationTime { get; set; }
}

/// <summary>调色板条目（对应 TOML 的 [[palette]] 表）。Lab 检索坐标由 RGB 纯函数重算，不持久化</summary>
public sealed record PaletteEntry(string Color, float Percentage);

/// <summary>
/// 素材参数元数据，对应 .hawk/metadata/&lt;hash&gt;.toml（唯一权威数据源，参与网盘同步）。
/// 宽高与调色板是「内容的纯函数」——同 hash 各平台计算结果必然一致，故直接入 TOML：
/// 一台设备计算、全平台（含未来 Rust 版）复用，免派生缓存与双写；同步冲突无语义风险。
/// </summary>
public sealed class ItemMetadata
{
    public List<PathEntry> Paths { get; } = new();
    public string? Url { get; set; }
    public List<string> Tags { get; set; } = new();
    public List<string> Categories { get; set; } = new();
    public int Star { get; set; }
    public string? Annotation { get; set; }

    /// <summary>图像宽（像素）；0 = 未知/非图像，索引时惰性补齐</summary>
    public int Width { get; set; }

    /// <summary>图像高（像素）；0 = 未知/非图像</summary>
    public int Height { get; set; }

    /// <summary>提炼的调色板（按占比降序，最多 10 色）；null = 未提炼，空表 = 已提炼但无有效像素（负缓存）</summary>
    public List<PaletteEntry>? Palette { get; set; }

    /// <summary>调色板算法版本（提炼时写入；与本机算法不一致的旧结果视为未提炼，触发重新提炼）</summary>
    public int PaletteVersion { get; set; }

    public PathEntry? FindPath(string path) => Paths.FirstOrDefault(p => p.Path == path);
}

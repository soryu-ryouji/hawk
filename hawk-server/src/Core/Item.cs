namespace Hawk.Server.Core;

/// <summary>item 的一个文件位置。Path 为相对库根目录路径；回收站位置以 .hawk/trash/ 开头。</summary>
public sealed class ItemLocation
{
    public required string Path { get; set; }
    public long Size { get; set; }
    public long ModificationTime { get; set; }

    public bool InTrash => LibraryPaths.IsInTrash(Path);

    /// <summary>对应的库内路径（回收站位置去掉前缀，即删除前的原路径）</summary>
    public string LibraryPath => InTrash ? LibraryPaths.TrashToLibraryPath(Path) : Path;
}

/// <summary>调色板中的一个颜色：RGB 用于展示与缓存，Lab 为预算的检索坐标</summary>
public sealed record PaletteColor(byte R, byte G, byte B, float Percentage, double LabL, double LabA, double LabB)
{
    public static PaletteColor FromRgb(byte r, byte g, byte b, float percentage)
    {
        var lab = ColorMath.RgbToLab(r, g, b);
        return new PaletteColor(r, g, b, percentage, lab.L, lab.A, lab.B);
    }

    public LabColor Lab => new(LabL, LabA, LabB);
}

/// <summary>
/// 内存索引中的 item。相同内容的文件共享一个 item，Locations 记录所有文件位置。
/// tags/star/annotation/url 以元数据为准，此处为查询用副本，由流水线单向同步。
/// </summary>
public sealed class Item
{
    public required string Id { get; init; }
    public List<ItemLocation> Locations { get; } = new();
    public string? Url { get; set; }
    public List<string> Tags { get; set; } = new();
    public List<string> Categories { get; set; } = new();
    public int Star { get; set; }
    public string? Annotation { get; set; }
    public int Width { get; set; }
    public int Height { get; set; }

    /// <summary>提炼的调色板（按占比降序，最多 10 个）；尚未提炼或不支持解码时为空</summary>
    public PaletteColor[] Palette { get; set; } = [];

    public bool HasLibraryLocations => Locations.Any(l => !l.InTrash);
    public bool HasTrashLocations => Locations.Any(l => l.InTrash);

    /// <summary>主位置：普通视图取首个库内位置，回收站视图取首个回收站位置</summary>
    public ItemLocation? MainLocation(bool trashView) =>
        trashView ? Locations.FirstOrDefault(l => l.InTrash) : Locations.FirstOrDefault(l => !l.InTrash);

    /// <summary>投影为 API 的 Item 对象。回收站视图的 paths 展示原库内路径（恢复目标）。</summary>
    public ItemDto ToDto(bool trashView)
    {
        var locations = Locations.Where(l => l.InTrash == trashView).ToList();
        var main = locations[0];
        var paths = locations.Select(l => trashView ? l.LibraryPath : l.Path).ToArray();

        return new ItemDto
        {
            Id = Id,
            Name = LibraryPaths.NameOf(main.LibraryPath),
            Ext = LibraryPaths.ExtOf(main.LibraryPath),
            Width = Width,
            Height = Height,
            Size = main.Size,
            Url = Url,
            Tags = Tags.ToArray(),
            Paths = paths,
            Folders = paths.Select(LibraryPaths.DirOf).Where(d => d != "").Distinct().ToArray(),
            Star = Star,
            Categories = Categories.ToArray(),
            Annotation = Annotation,
            ModificationTime = main.ModificationTime,
            Palette = Palette.Select(p => new PaletteColorDto { Color = ColorMath.ToHex(p.R, p.G, p.B), Percentage = p.Percentage }).ToArray(),
        };
    }
}

/// <summary>API 的调色板颜色项</summary>
public sealed record PaletteColorDto
{
    /// <summary># 前缀小写 hex，如 "#344441"</summary>
    public required string Color { get; init; }

    /// <summary>像素覆盖占比（0–100，1 位小数）</summary>
    public float Percentage { get; init; }
}

/// <summary>API 的 Item 对象（字段命名经全局 snake_case 策略序列化）</summary>
/// <summary>网格骨架：虚拟布局所需的最低限度信息（ItemDto 的同序轻量投影）</summary>
public sealed record ItemSkeletonDto
{
    public required string Id { get; init; }
    public int Width { get; init; }
    public int Height { get; init; }
    public int Star { get; init; }
}

public sealed record ItemDto
{
    public required string Id { get; init; }
    public required string Name { get; init; }
    public required string Ext { get; init; }
    public int Width { get; init; }
    public int Height { get; init; }
    public long Size { get; init; }
    public string? Url { get; init; }
    public string[] Tags { get; init; } = [];
    public string[] Categories { get; init; } = [];
    public required string[] Paths { get; init; }
    public required string[] Folders { get; init; }
    public int Star { get; init; }
    public string? Annotation { get; init; }
    public long ModificationTime { get; init; }
    public PaletteColorDto[] Palette { get; init; } = [];
}

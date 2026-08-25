using Hawk.Server.Core;

namespace Hawk.Server.Tests;

public class ItemTests
{
    private static Item BuildItem()
    {
        var item = new Item { Id = "hash1" };
        item.Tags.AddRange(["nature"]);
        item.Star = 4;
        item.Annotation = "备注";
        item.Url = "https://example.com/x.jpg";
        item.Width = 1920;
        item.Height = 1080;
        item.Locations.Add(new ItemLocation { Path = "posters/2024/cat.jpg", Size = 100, ModificationTime = 1000 });
        item.Locations.Add(new ItemLocation { Path = "icons/cat.jpg", Size = 100, ModificationTime = 2000 });
        return item;
    }

    [Fact]
    public void 普通视图_主位置为首个库内位置()
    {
        var dto = BuildItem().ToDto(trashView: false);

        Assert.Equal("cat", dto.Name);
        Assert.Equal("jpg", dto.Ext);
        Assert.Equal(["posters/2024/cat.jpg", "icons/cat.jpg"], dto.Paths);
        Assert.Equal(["posters/2024", "icons"], dto.Folders);
        Assert.Equal(1000, dto.ModificationTime); // 主位置的 mtime
        Assert.Equal(1920, dto.Width);
        Assert.Equal("hash1", dto.Id);
    }

    [Fact]
    public void 回收站视图_paths展示原库内路径()
    {
        var item = BuildItem();
        item.Locations.Add(new ItemLocation { Path = ".hawk/trash/old/cat.jpg", Size = 100, ModificationTime = 3000 });

        var dto = item.ToDto(trashView: true);
        Assert.Equal(["old/cat.jpg"], dto.Paths); // 恢复目标，不含 .hawk/trash/ 前缀
        Assert.Equal("cat", dto.Name);
        Assert.Equal(["old"], dto.Folders);
    }

    [Fact]
    public void 普通视图不包含回收站位置()
    {
        var item = BuildItem();
        item.Locations.Add(new ItemLocation { Path = ".hawk/trash/old/cat.jpg", Size = 100, ModificationTime = 3000 });

        var dto = item.ToDto(trashView: false);
        Assert.DoesNotContain(dto.Paths, p => p.Contains(".hawk"));
    }

    [Fact]
    public void 根目录文件的folders为空()
    {
        var item = new Item { Id = "h" };
        item.Locations.Add(new ItemLocation { Path = "root.png", Size = 1, ModificationTime = 1 });

        Assert.Empty(item.ToDto(trashView: false).Folders);
    }

    [Fact]
    public void 视图位置判定()
    {
        var item = new Item { Id = "h" };
        Assert.False(item.HasLibraryLocations);
        Assert.False(item.HasTrashLocations);

        item.Locations.Add(new ItemLocation { Path = ".hawk/trash/a.png", Size = 1, ModificationTime = 1 });
        Assert.False(item.HasLibraryLocations);
        Assert.True(item.HasTrashLocations);
        Assert.Null(item.MainLocation(trashView: false));

        item.Locations.Add(new ItemLocation { Path = "a.png", Size = 1, ModificationTime = 1 });
        Assert.True(item.HasLibraryLocations);
        Assert.Equal("a.png", item.MainLocation(trashView: false)!.Path);
    }
}

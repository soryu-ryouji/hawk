using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;

namespace Hawk.Server.Tests;

public class MetadataStoreTests
{
    private const string Hash1 = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
    private const string Hash2 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    private readonly TempDir _dir = new();

    private (LibraryPaths paths, MetadataStore store) CreateStore()
    {
        var paths = new LibraryPaths(_dir.Root);
        paths.EnsureLayout();
        return (paths, new MetadataStore(paths, NullLogger<MetadataStore>.Instance));
    }

    [Fact]
    public void 保存后重载_字段完整往返()
    {
        var (paths, store) = CreateStore();
        var meta = new ItemMetadata
        {
            Url = "https://example.com/photo.jpg",
            Tags = new List<string> { "nature", "中文标签" },
            Star = 4,
            Annotation = "含\"引号\"、\\反斜杠、\n换行",
        };
        meta.Paths.Add(new PathEntry { Path = "插画/角色\"一号\".png", Size = 245760, ModificationTime = 1700000000000 });
        meta.Paths.Add(new PathEntry { Path = "备份/copy.png", Size = 245760, ModificationTime = 1700000000000 });
        store.Save(Hash1, meta);

        // 模拟重启：新实例从磁盘加载
        var reloaded = new MetadataStore(paths, NullLogger<MetadataStore>.Instance);
        Assert.True(reloaded.TryGet(Hash1, out var loaded));
        Assert.Equal(meta.Url, loaded.Url);
        Assert.Equal(meta.Tags, loaded.Tags);
        Assert.Equal(meta.Star, loaded.Star);
        Assert.Equal(meta.Annotation, loaded.Annotation);
        Assert.Equal(2, loaded.Paths.Count);
        Assert.Equal("插画/角色\"一号\".png", loaded.Paths[0].Path);
        Assert.Equal(245760, loaded.Paths[0].Size);
        Assert.Equal(1700000000000, loaded.Paths[0].ModificationTime);
    }

    [Fact]
    public void 缺省字段不写入文件()
    {
        var (paths, store) = CreateStore();
        var meta = new ItemMetadata();
        meta.Paths.Add(new PathEntry { Path = "a.png", Size = 1, ModificationTime = 2 });
        store.Save(Hash1, meta);

        var toml = File.ReadAllText(Path.Combine(paths.MetadataDir, Hash1 + ".toml"));
        Assert.DoesNotContain("url", toml);
        Assert.DoesNotContain("tags", toml);
        Assert.DoesNotContain("star", toml);
        Assert.DoesNotContain("annotation", toml);
        Assert.Contains("[[paths]]", toml);
    }

    [Fact]
    public void 路径反查表随保存与删除更新()
    {
        var (_, store) = CreateStore();
        var meta = new ItemMetadata();
        meta.Paths.Add(new PathEntry { Path = "a.png", Size = 1, ModificationTime = 2 });
        store.Save(Hash1, meta);
        Assert.Equal(Hash1, store.FindHashByPath("a.png"));

        // 路径迁移到另一个 hash
        var meta2 = new ItemMetadata();
        meta2.Paths.Add(new PathEntry { Path = "a.png", Size = 1, ModificationTime = 2 });
        store.Save(Hash2, meta2);
        store.Delete(Hash1);
        Assert.Equal(Hash2, store.FindHashByPath("a.png"));

        store.Delete(Hash2);
        Assert.Null(store.FindHashByPath("a.png"));
        Assert.False(store.TryGet(Hash2, out _));
    }

    [Fact]
    public void 只识别64位hex命名的文件_同步冲突副本被忽略()
    {
        var (paths, store) = CreateStore();
        var meta = new ItemMetadata();
        meta.Paths.Add(new PathEntry { Path = "a.png", Size = 1, ModificationTime = 2 });
        store.Save(Hash1, meta);

        // 网盘同步冲突副本与非 hex 命名
        File.WriteAllText(Path.Combine(paths.MetadataDir, Hash1 + ".sync-conflict-20250101.toml"), "star = 5");
        File.WriteAllText(Path.Combine(paths.MetadataDir, "notes.toml"), "star = 5");

        var reloaded = new MetadataStore(paths, NullLogger<MetadataStore>.Instance);
        Assert.True(reloaded.TryGet(Hash1, out var loaded));
        Assert.Equal(0, loaded.Star); // 冲突副本的 star=5 不生效
    }

    [Fact]
    public void 损坏的元数据文件被跳过而不中断加载()
    {
        var (paths, store) = CreateStore();
        var meta = new ItemMetadata();
        meta.Paths.Add(new PathEntry { Path = "a.png", Size = 1, ModificationTime = 2 });
        store.Save(Hash1, meta);

        File.WriteAllText(Path.Combine(paths.MetadataDir, Hash2 + ".toml"), "not [ valid toml");

        var reloaded = new MetadataStore(paths, NullLogger<MetadataStore>.Instance);
        Assert.True(reloaded.TryGet(Hash1, out _));
        Assert.False(reloaded.TryGet(Hash2, out _));
    }
}

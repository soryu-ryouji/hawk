using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;

namespace Hawk.Server.Tests;

public class MetadataStoreTests
{
    private const string Hash1 = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
    private const string Hash2 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    private readonly TempDir _dir = new();

    private (LibraryPaths paths, IndexDb db, MetadataStore store) CreateStore(IndexDb? db = null)
    {
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();
        db ??= new IndexDb(paths, NullLogger<IndexDb>.Instance);
        return (paths, db, new MetadataStore(paths, db, NullLogger<MetadataStore>.Instance));
    }

    private static ItemMetadata NewMeta(params string[] paths)
    {
        var meta = new ItemMetadata();
        foreach (var p in paths)
        {
            meta.Paths.Add(new PathEntry { Path = p, Size = 1, ModificationTime = 2 });
        }

        return meta;
    }

    [Fact]
    public void 保存后重载_字段完整往返_缓存与TOML两条路径()
    {
        var (paths, db, store) = CreateStore();
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

        // 路径一：缓存已注水 → 新实例从 SQLite 注水
        var fromCache = new MetadataStore(paths, db, NullLogger<MetadataStore>.Instance);
        AssertFields(fromCache);

        // 路径二：缓存清空（未注水）→ 回退 TOML 全量解析
        db.Clear();
        Assert.False(db.IsHydrated);
        var fromToml = new MetadataStore(paths, db, NullLogger<MetadataStore>.Instance);
        AssertFields(fromToml);
        Assert.True(db.IsHydrated); // 回退路径顺带重建了缓存

        void AssertFields(MetadataStore s)
        {
            Assert.True(s.TryGet(Hash1, out var loaded));
            Assert.Equal(meta.Url, loaded.Url);
            Assert.Equal(meta.Tags, loaded.Tags);
            Assert.Equal(meta.Star, loaded.Star);
            Assert.Equal(meta.Annotation, loaded.Annotation);
            Assert.Equal(2, loaded.Paths.Count);
            Assert.Equal("插画/角色\"一号\".png", loaded.Paths[0].Path);
            Assert.Equal(245760, loaded.Paths[0].Size);
            Assert.Equal(1700000000000, loaded.Paths[0].ModificationTime);
        }
    }

    [Fact]
    public void 缺省字段不写入文件()
    {
        var (paths, _, store) = CreateStore();
        store.Save(Hash1, NewMeta("a.png"));

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
        var (_, _, store) = CreateStore();
        store.Save(Hash1, NewMeta("a.png"));
        Assert.Equal(Hash1, store.FindHashByPath("a.png"));

        // 路径迁移到另一个 hash
        store.Save(Hash2, NewMeta("a.png"));
        store.Delete(Hash1);
        Assert.Equal(Hash2, store.FindHashByPath("a.png"));

        store.Delete(Hash2);
        Assert.Null(store.FindHashByPath("a.png"));
        Assert.False(store.TryGet(Hash2, out _));
    }

    [Fact]
    public void 只识别64位hex命名的文件_同步冲突副本被忽略()
    {
        var (paths, db, store) = CreateStore();
        store.Save(Hash1, NewMeta("a.png"));

        // 网盘同步冲突副本与非 hex 命名
        File.WriteAllText(Path.Combine(paths.MetadataDir, Hash1 + ".sync-conflict-20250101.toml"), "star = 5");
        File.WriteAllText(Path.Combine(paths.MetadataDir, "notes.toml"), "star = 5");

        // 强制走 TOML 回退路径验证过滤逻辑（缓存路径下冲突副本根本进不了缓存）
        db.Clear();
        var reloaded = new MetadataStore(paths, db, NullLogger<MetadataStore>.Instance);
        Assert.True(reloaded.TryGet(Hash1, out var loaded));
        Assert.Equal(0, loaded.Star); // 冲突副本的 star=5 不生效
    }

    [Fact]
    public void 损坏的元数据文件被跳过而不中断加载()
    {
        var (paths, db, store) = CreateStore();
        store.Save(Hash1, NewMeta("a.png"));

        File.WriteAllText(Path.Combine(paths.MetadataDir, Hash2 + ".toml"), "not [ valid toml");

        db.Clear();
        var reloaded = new MetadataStore(paths, db, NullLogger<MetadataStore>.Instance);
        Assert.True(reloaded.TryGet(Hash1, out _));
        Assert.False(reloaded.TryGet(Hash2, out _));
    }

    [Fact]
    public void 对账载入_外部修改的TOML进入内存与缓存()
    {
        var (paths, db, store) = CreateStore();
        store.Save(Hash1, NewMeta("a.png"));

        // 外部（另一台机器经网盘同步）直接改写 TOML
        var file = Path.Combine(paths.MetadataDir, Hash1 + ".toml");
        File.WriteAllText(file, "tags = [\"外部标签\"]\nstar = 5\n");
        var mtime = LibraryPaths.ToUnixMs(File.GetLastWriteTimeUtc(file));

        Assert.True(store.ApplyExternalToml(Hash1, file, mtime));
        Assert.True(store.TryGet(Hash1, out var meta));
        Assert.Equal(new List<string> { "外部标签" }, meta.Tags);
        Assert.Equal(5, meta.Star);
        Assert.Empty(meta.Paths); // 外部 TOML 无 paths 段 → 以文件为准，不合并

        // 缓存同样被更新：新实例注水即见
        var reloaded = new MetadataStore(paths, db, NullLogger<MetadataStore>.Instance);
        Assert.True(reloaded.TryGet(Hash1, out var loaded));
        Assert.Equal(new List<string> { "外部标签" }, loaded.Tags);
    }

    [Fact]
    public void 对账载入_解析失败返回false且状态不被清空()
    {
        var (paths, db, store) = CreateStore();
        store.Save(Hash1, NewMeta("a.png"));

        var file = Path.Combine(paths.MetadataDir, Hash1 + ".toml");
        File.WriteAllText(file, "not [ valid toml");
        var mtime = LibraryPaths.ToUnixMs(File.GetLastWriteTimeUtc(file));

        Assert.False(store.ApplyExternalToml(Hash1, file, mtime));
        Assert.True(store.TryGet(Hash1, out var meta));
        Assert.Single(meta.Paths); // 内存保持原状，下轮对账重试
    }

    [Fact]
    public void 对账清空_TOML消失后字段清空且缓存同步()
    {
        var (paths, db, store) = CreateStore();
        var meta = new ItemMetadata { Star = 3, Annotation = "备注" };
        meta.Tags.Add("标签");
        meta.Paths.Add(new PathEntry { Path = "a.png", Size = 1, ModificationTime = 2 });
        store.Save(Hash1, meta);

        File.Delete(Path.Combine(paths.MetadataDir, Hash1 + ".toml"));
        store.ClearExternal(Hash1);

        Assert.True(store.TryGet(Hash1, out var cleared));
        Assert.Equal(0, cleared.Star);
        Assert.Null(cleared.Annotation);
        Assert.Empty(cleared.Tags);
        Assert.Empty(cleared.Paths); // 等价于重启后无元数据的语义

        var reloaded = new MetadataStore(paths, db, NullLogger<MetadataStore>.Instance);
        Assert.True(reloaded.TryGet(Hash1, out var loaded));
        Assert.Equal(0, loaded.Star);
    }
}

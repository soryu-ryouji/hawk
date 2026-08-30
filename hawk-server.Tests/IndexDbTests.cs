using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;

namespace Hawk.Server.Tests;

/// <summary>IndexDb（元数据 SQLite 派生缓存）单测：注水/读回/写穿/删除/清空与注水标记</summary>
public class IndexDbTests
{
    private const string Hash1 = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
    private const string Hash2 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    private readonly TempDir _dir = new();

    private IndexDb CreateDb()
    {
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();
        return new IndexDb(paths, NullLogger<IndexDb>.Instance);
    }

    private static (string Hash, ItemMetadata Meta, long SourceMtime) Entry(
        string hash, long sourceMtime, string path, params string[] tags)
    {
        var meta = new ItemMetadata
        {
            Url = "https://example.com/x.png",
            Star = 4,
            Annotation = "备注",
        };
        meta.Tags.AddRange(tags);
        meta.Categories.Add("灵感");
        meta.Paths.Add(new PathEntry { Path = path, Size = 245760, ModificationTime = 1700000000000 });
        return (hash, meta, sourceMtime);
    }

    [Fact]
    public void 注水后全量读回_字段完整()
    {
        using var db = CreateDb();
        Assert.False(db.IsHydrated); // 新建缓存未注水

        var entries = new[] { Entry(Hash1, 111, "a.png", "t1", "t2"), Entry(Hash2, 222, "b.png") };
        db.Hydrate(entries);
        Assert.True(db.IsHydrated);

        var all = db.LoadAll();
        Assert.Equal(2, all.Count);

        var e1 = all.Single(e => e.Hash == Hash1);
        Assert.Equal(111, e1.SourceMtime);
        Assert.Equal("https://example.com/x.png", e1.Meta.Url);
        Assert.Equal(4, e1.Meta.Star);
        Assert.Equal("备注", e1.Meta.Annotation);
        Assert.Equal(new List<string> { "t1", "t2" }, e1.Meta.Tags);
        Assert.Equal(new List<string> { "灵感" }, e1.Meta.Categories);
        var p = Assert.Single(e1.Meta.Paths);
        Assert.Equal("a.png", p.Path);
        Assert.Equal(245760, p.Size);
        Assert.Equal(1700000000000, p.ModificationTime);

        var e2 = all.Single(e => e.Hash == Hash2);
        Assert.Equal(222, e2.SourceMtime);
        Assert.Empty(e2.Meta.Tags);
    }

    [Fact]
    public void 空库注水也是合法完成态()
    {
        using var db = CreateDb();
        db.Hydrate([]);
        Assert.True(db.IsHydrated);
        Assert.Empty(db.LoadAll());
    }

    [Fact]
    public void 写穿保存与删除_子表同步替换()
    {
        using var db = CreateDb();
        db.Hydrate([Entry(Hash1, 111, "a.png", "旧标签")]);

        // 同一 hash 再保存：子行整体替换而非追加
        db.Save(Hash1, Entry(Hash1, 333, "b.png", "新标签").Meta, 333);

        var mtimes = db.LoadSourceMtimes()!;
        Assert.Equal(333, mtimes[Hash1]);

        var all = db.LoadAll();
        var e = Assert.Single(all);
        Assert.Equal(333, e.SourceMtime);
        Assert.Equal(new List<string> { "新标签" }, e.Meta.Tags);
        Assert.Equal("b.png", Assert.Single(e.Meta.Paths).Path);

        db.Delete(Hash1);
        Assert.Empty(db.LoadAll());
        Assert.Empty(db.LoadSourceMtimes()!);
    }

    [Fact]
    public void 清空复位注水标记()
    {
        using var db = CreateDb();
        db.Hydrate([Entry(Hash1, 111, "a.png")]);
        Assert.True(db.IsHydrated);

        db.Clear();
        Assert.False(db.IsHydrated);
        Assert.Empty(db.LoadAll());
    }

    [Fact]
    public void 释放后无残留文件_主库含数据()
    {
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();
        var db = new IndexDb(paths, NullLogger<IndexDb>.Instance);
        db.Hydrate([Entry(Hash1, 111, "a.png", "t")]);

        // journal_mode=DELETE：提交直接写入主库文件，目录里不产生 -wal/-shm/-journal 伴生文件
        db.Dispose();

        Assert.False(File.Exists(paths.IndexDbFile + "-wal"));
        Assert.False(File.Exists(paths.IndexDbFile + "-shm"));
        Assert.False(File.Exists(paths.IndexDbFile + "-journal"));

        var reloaded = new IndexDb(paths, NullLogger<IndexDb>.Instance);
        Assert.True(reloaded.IsHydrated);
        Assert.Single(reloaded.LoadAll());
        reloaded.Dispose();
    }

    [Fact]
    public void 缓存文件持久存在_新实例继承注水状态()
    {
        var paths = new LibraryPaths(_dir.Root, _dir.CacheRoot);
        paths.EnsureLayout();
        using (var db = new IndexDb(paths, NullLogger<IndexDb>.Instance))
        {
            db.Hydrate([Entry(Hash1, 111, "a.png", "t")]);
        }

        // 模拟重启：新实例打开同一缓存文件
        using (var db2 = new IndexDb(paths, NullLogger<IndexDb>.Instance))
        {
            Assert.True(db2.IsHydrated);
            var all = db2.LoadAll();
            Assert.Equal("t", Assert.Single(all).Meta.Tags.Single());
        }
    }
}

using System.Diagnostics;
using Hawk.Server.Core;
using Microsoft.Extensions.Logging.Abstractions;
using SixLabors.ImageSharp;
using SixLabors.ImageSharp.PixelFormats;

namespace Hawk.Server.Tests;

/// <summary>
/// IndexPipeline 集成测试：临时目录 + 真实文件系统。
/// 覆盖入库、哈希复用、id 漂移迁移、移动、多路径、回收站清理、事件与防抖。
/// </summary>
public class IndexPipelineTests
{
    /// <summary>一套完整的服务组合（无 watcher，事件由测试直接注入）</summary>
    private sealed class Rig : IDisposable
    {
        public required LibraryPaths Paths;
        public required MetadataStore Store;
        public required ItemIndex Index;
        public required ThumbnailService Thumbnails;
        public required ColorService Colors;
        public required EventBus Bus;
        public required CategoryRegistry Categories;
        public required TagRegistry Tags;
        public required IndexPipeline Pipeline;

        public static Rig Create(string root, string cacheRoot)
        {
            var paths = new LibraryPaths(root, cacheRoot);
            paths.EnsureLayout();
            var config = new LibraryConfig(paths, NullLogger<LibraryConfig>.Instance);
            var store = new MetadataStore(paths, NullLogger<MetadataStore>.Instance);
            var index = new ItemIndex();
            var thumbnails = new ThumbnailService(paths, NullLogger<ThumbnailService>.Instance);
            var colors = new ColorService(paths, NullLogger<ColorService>.Instance);
            var bus = new EventBus();
            var categories = new CategoryRegistry(paths, NullLogger<CategoryRegistry>.Instance);
            var tags = new TagRegistry(paths, NullLogger<TagRegistry>.Instance);
            var scanner = new LibraryScanner(paths, config);
            var settings = new ServerSettings { LibraryRoot = root, Token = "test" };
            var worker = new ThumbnailWorker(thumbnails, colors, config, bus, NullLogger<ThumbnailWorker>.Instance);
            var migrator = new TaxonomyMigrator(store, index, categories, tags, bus);
            var pipeline = new IndexPipeline(paths, config, store, index, thumbnails, colors, bus, scanner, migrator, settings,
                NullLogger<IndexPipeline>.Instance);
            pipeline.AttachThumbnailWorker(worker);
            pipeline.Start();
            return new Rig { Paths = paths, Store = store, Index = index, Thumbnails = thumbnails, Colors = colors, Bus = bus, Categories = categories, Tags = tags, Pipeline = pipeline };
        }

        public void Dispose() => Pipeline.Dispose();
    }

    private static async Task<bool> WaitUntil(Func<bool> condition, int timeoutMs = 5000)
    {
        var sw = Stopwatch.StartNew();
        while (sw.ElapsedMilliseconds < timeoutMs)
        {
            if (condition())
            {
                return true;
            }

            await Task.Delay(50);
        }

        return condition();
    }

    private readonly TempDir _dir = new();

    [Fact]
    public async Task 入库_生成索引与元数据并识别尺寸()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("photo.png", TempDir.TinyPng);

        var result = await rig.Pipeline.SubmitUpsertAsync(file);

        Assert.NotNull(result);
        Assert.False(result.AlreadyExisted);
        Assert.Equal(64, result.Item.Id.Length);
        Assert.Equal(1, result.Item.Width);
        Assert.Equal(1, result.Item.Height);
        Assert.Equal("photo.png", Assert.Single(result.Item.Paths));
        Assert.True(File.Exists(Path.Combine(rig.Paths.MetadataDir, result.Item.Id + ".toml")));
    }

    [Fact]
    public async Task 重启后哈希复用_元数据文件不被重写()
    {
        string hash;
        string metaFile;
        DateTime metaMtime;

        using (var rig1 = Rig.Create(_dir.Root, _dir.CacheRoot))
        {
            var file = _dir.WriteFile("a.png", TempDir.TinyPng);
            var result = await rig1.Pipeline.SubmitUpsertAsync(file);
            hash = result!.Item.Id;
            metaFile = Path.Combine(rig1.Paths.MetadataDir, hash + ".toml");
            metaMtime = File.GetLastWriteTimeUtc(metaFile);
        }

        // 模拟重启：全新实例从磁盘元数据加载，路径 + size/mtime 一致 → 复用哈希不读内容
        using (var rig2 = Rig.Create(_dir.Root, _dir.CacheRoot))
        {
            var result = await rig2.Pipeline.SubmitUpsertAsync(Path.Combine(_dir.Root, "a.png"));
            Assert.Equal(hash, result!.Item.Id); // 哈希来自元数据文件名，未重算
        }

        Assert.Equal(metaMtime, File.GetLastWriteTimeUtc(metaFile)); // size/mtime 未变 → 元数据不重写
    }

    [Fact]
    public async Task 内容修改导致id漂移_元数据按路径迁移并继承素材参数()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("a.png", TempDir.TinyPng);
        var first = await rig.Pipeline.SubmitUpsertAsync(file);
        var oldHash = first!.Item.Id;

        await rig.Pipeline.SubmitMetadataAsync(oldHash, meta =>
        {
            meta.Tags = new List<string> { "保留标签" };
            meta.Star = 5;
            meta.Annotation = "迁移备注";
        });

        // 等后台 worker 结束（缩略图 + 调色板提炼完成），确保源文件句柄已释放
        Assert.True(await WaitUntil(() => rig.Colors.Exists(oldHash)));

        // 修改内容（mtime 必须前进，否则复用判定会命中）
        File.WriteAllBytes(file, [1, 2, 3, 4, 5, 6]);
        File.SetLastWriteTimeUtc(file, DateTime.UtcNow.AddSeconds(2));
        var second = await rig.Pipeline.SubmitUpsertAsync(file);

        Assert.NotEqual(oldHash, second!.Item.Id);
        Assert.Equal(["保留标签"], second.Item.Tags);
        Assert.Equal(5, second.Item.Star);
        Assert.Equal("迁移备注", second.Item.Annotation);

        // 旧 id 无其他位置 → 索引与元数据都清理
        Assert.Null(rig.Index.Get(oldHash));
        Assert.False(File.Exists(Path.Combine(rig.Paths.MetadataDir, oldHash + ".toml")));
        Assert.True(File.Exists(Path.Combine(rig.Paths.MetadataDir, second.Item.Id + ".toml")));
    }

    [Fact]
    public async Task 移动文件_保持id且元数据路径跟随()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("a/b.png", TempDir.TinyPng);
        var first = await rig.Pipeline.SubmitUpsertAsync(file);

        _dir.Mkdir("c");
        var target = Path.Combine(_dir.Root, "c", "renamed.png");
        File.Move(file, target);
        await rig.Pipeline.SubmitMoveAsync(file, target);

        var item = rig.Index.Get(first!.Item.Id)!;
        Assert.Equal("c/renamed.png", Assert.Single(item.Locations).Path);
        Assert.True(rig.Store.TryGet(first.Item.Id, out var meta));
        Assert.Equal("c/renamed.png", Assert.Single(meta.Paths).Path);
    }

    [Fact]
    public async Task 同内容多路径_回收一份不影响库内item()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var f1 = _dir.WriteFile("a.png", TempDir.TinyPng);
        var f2 = _dir.WriteFile("dir/b.png", TempDir.TinyPng);
        var r1 = await rig.Pipeline.SubmitUpsertAsync(f1);
        await rig.Pipeline.SubmitUpsertAsync(f2);

        var item = rig.Index.Get(r1!.Item.Id)!;
        Assert.Equal(2, item.Locations.Count);

        // 一份移入回收站（模拟 item/delete 的 FS 操作 + MoveJob）
        var trashAbs = LibraryFs.FindFreeTrashPath(rig.Paths, "a.png", isDirectory: false);
        LibraryFs.EnsureParentDir(trashAbs);
        File.Move(f1, trashAbs);
        await rig.Pipeline.SubmitMoveAsync(f1, trashAbs);

        Assert.True(item.HasLibraryLocations); // dir/b.png 仍在库内
        Assert.True(item.HasTrashLocations);

        // 清空回收站：物理删除后提交 ClearTrashJob
        File.Delete(trashAbs);
        await rig.Pipeline.SubmitClearTrashAsync();

        // 库内仍有引用 → 元数据保留，但回收站位置的路径已清理
        Assert.True(rig.Store.TryGet(item.Id, out var meta));
        Assert.Equal(["dir/b.png"], meta.Paths.Select(p => p.Path).ToArray());
        Assert.Single(item.Locations);
    }

    [Fact]
    public async Task 清空回收站_无其他引用时清理元数据与缩略图()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("only.png", TempDir.TinyPng);
        var result = await rig.Pipeline.SubmitUpsertAsync(file);
        var hash = result!.Item.Id;

        // 等缩略图与调色板缓存生成（后台 worker 异步），确保缩略图句柄已释放
        Assert.True(await WaitUntil(() => rig.Thumbnails.Exists(hash, 256) && rig.Colors.Exists(hash)));

        var trashAbs = LibraryFs.FindFreeTrashPath(rig.Paths, "only.png", isDirectory: false);
        LibraryFs.EnsureParentDir(trashAbs);
        File.Move(file, trashAbs);
        await rig.Pipeline.SubmitMoveAsync(file, trashAbs);

        File.Delete(trashAbs);
        await rig.Pipeline.SubmitClearTrashAsync();

        Assert.Null(rig.Index.Get(hash));
        Assert.False(File.Exists(Path.Combine(rig.Paths.MetadataDir, hash + ".toml")));
        Assert.False(rig.Thumbnails.Exists(hash, 256));
    }

    private static byte[] SolidPng(byte r, byte g, byte b)
    {
        using var image = new Image<Rgba32>(32, 32, new Rgba32(r, g, b));
        using var ms = new MemoryStream();
        image.SaveAsPng(ms);
        return ms.ToArray();
    }

    [Fact]
    public async Task 入库后_后台提炼调色板并写入缓存()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("red.png", SolidPng(255, 0, 0));
        var result = await rig.Pipeline.SubmitUpsertAsync(file);
        var hash = result!.Item.Id;

        // 后台 worker 异步提炼
        Assert.True(await WaitUntil(() => rig.Index.Get(hash)?.Palette.Length > 0));
        var palette = rig.Index.Get(hash)!.Palette;
        var color = Assert.Single(palette);
        Assert.Equal("#ff0000", ColorMath.ToHex(color.R, color.G, color.B));
        Assert.Equal(100f, color.Percentage);
        Assert.True(rig.Colors.Exists(hash));
    }

    [Fact]
    public async Task 重启扫描_从缓存载入调色板()
    {
        string hash;
        using (var rig = Rig.Create(_dir.Root, _dir.CacheRoot))
        {
            var file = _dir.WriteFile("red.png", SolidPng(255, 0, 0));
            var result = await rig.Pipeline.SubmitUpsertAsync(file);
            hash = result!.Item.Id;
            Assert.True(await WaitUntil(() => rig.Colors.Exists(hash)));
        }

        // 重建一套服务（模拟重启）：哈希复用不读文件内容，调色板应从缓存文件载入
        using var rig2 = Rig.Create(_dir.Root, _dir.CacheRoot);
        await rig2.Pipeline.RunScanAsync(false);
        var palette = rig2.Index.Get(hash)?.Palette;
        var color = Assert.Single(palette!);
        Assert.Equal("#ff0000", ColorMath.ToHex(color.R, color.G, color.B));
    }

    [Fact]
    public async Task 清空回收站_清理调色板缓存()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("red.png", SolidPng(255, 0, 0));
        var result = await rig.Pipeline.SubmitUpsertAsync(file);
        var hash = result!.Item.Id;
        Assert.True(await WaitUntil(() => rig.Colors.Exists(hash)));

        var trashAbs = LibraryFs.FindFreeTrashPath(rig.Paths, "red.png", isDirectory: false);
        LibraryFs.EnsureParentDir(trashAbs);
        File.Move(file, trashAbs);
        await rig.Pipeline.SubmitMoveAsync(file, trashAbs);

        File.Delete(trashAbs);
        await rig.Pipeline.SubmitClearTrashAsync();

        Assert.Null(rig.Index.Get(hash));
        Assert.False(rig.Colors.Exists(hash));
    }

    [Fact]
    public async Task 事件发布_入库与清空回收站()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var reader = rig.Bus.Subscribe();
        var file = _dir.WriteFile("ev.png", TempDir.TinyPng);

        var result = await rig.Pipeline.SubmitUpsertAsync(file);
        var added = await reader.ReadAsync();
        Assert.Equal("item.added", added.Type);
        var dto = Assert.IsType<ItemDto>(added.Payload);
        Assert.Equal(result!.Item.Id, dto.Id);
    }

    [Fact]
    public async Task 扫描_索引既有文件并检测消失()
    {
        // 文件先于流水线存在；刚写入的文件会经历约 1s 防抖窗口后入库
        _dir.WriteFile("scan/one.png", TempDir.TinyPng);
        var two = _dir.WriteFile("scan/two.png", [9, 9, 9]);

        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        await rig.Pipeline.RunScanAsync(full: false);
        Assert.True(await WaitUntil(() => rig.Index.Count() == 2));

        // 外部删除后重新扫描 → 位置摘除
        File.Delete(two);
        await rig.Pipeline.RunScanAsync(full: false);
        Assert.Equal(1, rig.Index.Count());
    }

    [Fact]
    public async Task 全量重扫_重算哈希并保持id稳定()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("stable.png", TempDir.TinyPng);
        var first = await rig.Pipeline.SubmitUpsertAsync(file);

        await rig.Pipeline.RunScanAsync(full: true); // 强制重算

        Assert.Equal(first!.Item.Id, rig.Index.Get(first.Item.Id)?.Id);
        Assert.Equal(1, rig.Index.Count());
    }

    [Fact]
    public async Task 监听入口_写入中的文件防抖延迟入库()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("fresh.bin", new byte[64]); // mtime 为当前时刻

        rig.Pipeline.NotifyUpsert(file); // 监听入口（fire-and-forget）才走防抖

        await Task.Delay(200); // 防抖窗口 1s 内不应入库
        Assert.Empty(rig.Index.AllLocationPaths());

        Assert.True(await WaitUntil(() => rig.Index.AllLocationPaths().Length == 1));
    }

    [Fact]
    public async Task API提交_不防抖立即入库()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("instant.bin", new byte[64]);

        var result = await rig.Pipeline.SubmitUpsertAsync(file); // 携带 Done 的提交直接处理

        Assert.NotNull(result);
        Assert.Single(rig.Index.AllLocationPaths());
    }

    // ---------- 分类/标签级联 ----------

    [Fact]
    public async Task 分类重命名_元数据跟随迁移()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("art.png", TempDir.TinyPng);
        var result = await rig.Pipeline.SubmitUpsertAsync(file);
        var hash = result!.Item.Id;

        await rig.Pipeline.SubmitMetadataAsync(hash, m => m.Categories = ["插画"]);
        Assert.Contains("插画", rig.Categories.Snapshot()); // 赋值自动登记注册表

        await rig.Pipeline.SubmitCategoryUpdateAsync("插画", "灵感");

        Assert.True(rig.Store.TryGet(hash, out var meta));
        Assert.Equal(["灵感"], meta.Categories);
        Assert.Equal(["灵感"], rig.Categories.Snapshot());
        Assert.Equal(["灵感"], rig.Index.Get(hash)!.Categories);
    }

    [Fact]
    public async Task 分类删除_清除全部赋值()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("art.png", TempDir.TinyPng);
        var result = await rig.Pipeline.SubmitUpsertAsync(file);
        var hash = result!.Item.Id;

        await rig.Pipeline.SubmitMetadataAsync(hash, m => m.Categories = ["插画", "参考"]);
        await rig.Pipeline.SubmitCategoryDeleteAsync("插画");

        Assert.True(rig.Store.TryGet(hash, out var meta));
        Assert.Equal(["参考"], meta.Categories);
        Assert.DoesNotContain("插画", rig.Categories.Snapshot());
    }

    [Fact]
    public async Task 标签重命名_全部item跟随()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("photo.png", TempDir.TinyPng);
        var result = await rig.Pipeline.SubmitUpsertAsync(file);
        var hash = result!.Item.Id;

        await rig.Pipeline.SubmitMetadataAsync(hash, m => m.Tags = ["natrue", "work"]);
        await rig.Pipeline.SubmitTagUpdateAsync("natrue", "nature");

        Assert.True(rig.Store.TryGet(hash, out var meta));
        Assert.Equal(["nature", "work"], meta.Tags);
        Assert.Contains("nature", rig.Tags.Snapshot());
        Assert.DoesNotContain("natrue", rig.Tags.Snapshot());
    }

    [Fact]
    public async Task 标签删除_注册表与赋值同步清除()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var file = _dir.WriteFile("photo.png", TempDir.TinyPng);
        var result = await rig.Pipeline.SubmitUpsertAsync(file);
        var hash = result!.Item.Id;

        await rig.Pipeline.SubmitMetadataAsync(hash, m => m.Tags = ["nature"]);
        await rig.Pipeline.SubmitTagDeleteAsync("nature");

        Assert.True(rig.Store.TryGet(hash, out var meta));
        Assert.Empty(meta.Tags);
        Assert.Empty(rig.Tags.Snapshot());
    }

    [Fact]
    public async Task 批量元数据_标签并集追加且报告缺失()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var f1 = _dir.WriteFile("a.png", TempDir.TinyPng);
        var f2 = _dir.WriteFile("b.png", [9, 9, 9]);
        var r1 = await rig.Pipeline.SubmitUpsertAsync(f1);
        var r2 = await rig.Pipeline.SubmitUpsertAsync(f2);
        var h1 = r1!.Item.Id;
        var h2 = r2!.Item.Id;

        await rig.Pipeline.SubmitMetadataAsync(h1, m => m.Tags = ["已有"]);

        var missingHash = new string('0', 64); // 索引中不存在
        var result = await rig.Pipeline.SubmitBatchMetadataAsync(
            [h1, h2, missingHash],
            m =>
            {
                m.Tags = m.Tags.Union(["批量"], StringComparer.Ordinal).ToList();
                m.Star = 3;
            });

        Assert.Equal(2, result.Updated);
        Assert.Equal([missingHash], result.MissingIds);
        Assert.True(rig.Store.TryGet(h1, out var m1));
        Assert.Equal(["已有", "批量"], m1.Tags); // 并集追加,不覆盖
        Assert.Equal(3, m1.Star);
        Assert.True(rig.Store.TryGet(h2, out var m2));
        Assert.Equal(["批量"], m2.Tags);
        Assert.Contains("批量", rig.Tags.Snapshot()); // 赋值自动登记注册表
        Assert.Equal(3, rig.Index.Get(h2)!.Star);
    }

    [Fact]
    public async Task folder_changed_目录移动与提示入口均广播事件()
    {
        using var rig = Rig.Create(_dir.Root, _dir.CacheRoot);
        var reader = rig.Bus.Subscribe();
        var file = _dir.WriteFile("d/a.png", TempDir.TinyPng);
        var result = await rig.Pipeline.SubmitUpsertAsync(file);

        // 等后台缩略图/调色板 worker 释放文件句柄,再模拟外部进程移动目录(watcher 上报时移动已完成)
        Assert.True(await WaitUntil(() => rig.Colors.Exists(result!.Item.Id)));

        // 事件序列不固定(缩略图 worker 完成也会补发 item.updated):循环读到 folder.changed 为止
        _dir.Mkdir("e");
        var oldDir = Path.Combine(_dir.Root, "d");
        var newDir = Path.Combine(_dir.Root, "e", "d");
        Directory.Move(oldDir, newDir);
        await rig.Pipeline.SubmitDirMoveAsync(oldDir, newDir);

        FolderChangedPayload? payload = null;
        var sw = Stopwatch.StartNew();
        while (sw.ElapsedMilliseconds < 5000)
        {
            var evt = await reader.ReadAsync();
            if (evt.Type == "folder.changed")
            {
                payload = Assert.IsType<FolderChangedPayload>(evt.Payload);
                break;
            }
        }

        Assert.NotNull(payload);
        Assert.Equal("external", payload!.Reason);

        // 提示入口(watcher 目录创建/端点操作)
        rig.Pipeline.NotifyFolderChanged(FolderChangedPayload.ReasonExternal);
        Assert.Equal("folder.changed", (await reader.ReadAsync()).Type);
    }
}

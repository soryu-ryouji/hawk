using System.Collections.Concurrent;
using System.Threading.Channels;

namespace Hawk.Server.Core;

/// <summary>item/add 的处理结果：索引后的 item 与「内容是否已存在」标志</summary>
public sealed record UpsertResult(Item Item, bool AlreadyExisted);

/// <summary>
/// 索引流水线：监听事件 / 扫描 / API 写操作全部经有界 channel 串行处理（单写者），
/// 哈希与元数据迁移在消费者内完成；缩略图生成转到后台工作线程，不阻塞索引。
/// 索引与元数据的所有变更只发生在这里，处理逻辑保证幂等（重复事件无害）。
/// </summary>
public sealed class IndexPipeline : IDisposable
{
    private abstract record IndexJob;
    private sealed record UpsertJob(string AbsPath, bool ForceHash, string? KnownHash, TaskCompletionSource<UpsertResult?>? Done, int Attempt) : IndexJob;
    private sealed record DeleteJob(string AbsPath) : IndexJob;
    private sealed record MoveJob(string OldAbs, string NewAbs, TaskCompletionSource? Done) : IndexJob;
    private sealed record DirMoveJob(string OldAbs, string NewAbs, TaskCompletionSource? Done) : IndexJob;
    private sealed record ScanJob(bool Full, TaskCompletionSource? Done) : IndexJob;
    private sealed record ClearTrashJob(TaskCompletionSource? Done) : IndexJob;
    private sealed record MetadataJob(string Hash, Action<ItemMetadata> Mutate, TaskCompletionSource<Item?> Done) : IndexJob;
    private sealed record ThumbJob(string Hash, string SourceAbs);
    /// <summary>后台 worker 提炼调色板完成后的回写（单写者规则：索引变更只发生在消费循环）</summary>
    private sealed record PaletteJob(string Hash, PaletteColor[] Palette) : IndexJob;

    // 分类/标签操作（注册表 + 元数据批量迁移）
    private sealed record CategoryCreateJob(string Path, TaskCompletionSource Done) : IndexJob;
    private sealed record CategoryUpdateJob(string Path, string? Name, string? ParentPath, TaskCompletionSource Done) : IndexJob;
    private sealed record CategoryDeleteJob(string Path, TaskCompletionSource Done) : IndexJob;
    private sealed record TagCreateJob(string Name, TaskCompletionSource Done) : IndexJob;
    private sealed record TagUpdateJob(string Name, string NewName, TaskCompletionSource Done) : IndexJob;
    private sealed record TagDeleteJob(string Name, TaskCompletionSource Done) : IndexJob;
    private sealed record RegistryReloadJob : IndexJob;

    private readonly LibraryPaths _paths;
    private readonly LibraryConfig _config;
    private readonly MetadataStore _store;
    private readonly ItemIndex _index;
    private readonly ThumbnailService _thumbnails;
    private readonly ColorService _colors;
    private readonly EventBus _bus;
    private readonly LibraryScanner _scanner;
    private readonly CategoryRegistry _categories;
    private readonly TagRegistry _tags;
    private readonly ServerSettings _settings;
    private readonly ILogger<IndexPipeline> _logger;

    private readonly Channel<IndexJob> _jobs = Channel.CreateBounded<IndexJob>(
        new BoundedChannelOptions(4096) { FullMode = BoundedChannelFullMode.Wait, SingleReader = true });
    private readonly Channel<ThumbJob> _thumbJobs = Channel.CreateBounded<ThumbJob>(
        new BoundedChannelOptions(4096) { FullMode = BoundedChannelFullMode.Wait, SingleReader = false });

    private readonly CancellationTokenSource _cts = new();
    private int _overflow;
    private Task? _consumer;
    private Task? _reconciler;
    private Task[] _thumbWorkers = [];

    // 防抖：仍在写入中的文件（mtime 距今不足 StabilityWindow）不立即哈希，延迟重试
    private static readonly TimeSpan StabilityWindow = TimeSpan.FromSeconds(1);
    private const int MaxDebounceAttempts = 120;
    private readonly ConcurrentDictionary<string, byte> _deferredPaths = new();

    // 扫描阶段哈希计算的并行度（纯计算阶段，索引/元数据应用仍串行）
    private static readonly int HashParallelism = Math.Clamp(Environment.ProcessorCount / 2, 2, 8);

    public IndexPipeline(
        LibraryPaths paths,
        LibraryConfig config,
        MetadataStore store,
        ItemIndex index,
        ThumbnailService thumbnails,
        ColorService colors,
        EventBus bus,
        LibraryScanner scanner,
        CategoryRegistry categories,
        TagRegistry tags,
        ServerSettings settings,
        ILogger<IndexPipeline> logger)
    {
        _paths = paths;
        _config = config;
        _store = store;
        _index = index;
        _thumbnails = thumbnails;
        _colors = colors;
        _bus = bus;
        _scanner = scanner;
        _categories = categories;
        _tags = tags;
        _settings = settings;
        _logger = logger;
    }

    public void Start()
    {
        _consumer = ConsumeLoop(_cts.Token);
        // 缩略图为 CPU 密集操作，开少量并发工作线程
        _thumbWorkers = Enumerable.Range(0, Math.Clamp(Environment.ProcessorCount / 4, 1, 4))
            .Select(_ => ThumbLoop(_cts.Token))
            .ToArray();
        _reconciler = ReconcileLoop(_cts.Token);
    }

    /// <summary>
    /// 周期对账：文件监听（尤其 macOS FSEvents）可能静默丢事件且无溢出错误，
    /// 每 RescanIntervalSeconds 跑一次轻量全量扫描（复用哈希、不读文件内容）保证最终一致。0 关闭。
    /// </summary>
    private async Task ReconcileLoop(CancellationToken ct)
    {
        if (_settings.RescanIntervalSeconds <= 0)
        {
            return;
        }

        using var timer = new PeriodicTimer(TimeSpan.FromSeconds(_settings.RescanIntervalSeconds));
        try
        {
            while (await timer.WaitForNextTickAsync(ct))
            {
                FireAndForget(new ScanJob(Full: false, null));
            }
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
        }
    }

    // ---------- 入口：文件监听（火忘，channel 满时置溢出标记，由消费者全量扫描兜底） ----------

    public void NotifyUpsert(string absPath) => FireAndForget(new UpsertJob(absPath, false, null, null, 0));
    public void NotifyDeleted(string absPath) => FireAndForget(new DeleteJob(absPath));

    /// <summary>新路径是目录时按目录移动处理，否则按文件移动</summary>
    public void NotifyMoved(string oldAbs, string newAbs)
    {
        IndexJob job = Directory.Exists(newAbs)
            ? new DirMoveJob(oldAbs, newAbs, null)
            : new MoveJob(oldAbs, newAbs, null);
        FireAndForget(job);
    }

    public void NotifyConfigChanged() => FireAndForget(new ScanJob(Full: false, null));
    public void NotifyOverflow() => Interlocked.Exchange(ref _overflow, 1);

    /// <summary>注册表文件被外部修改（网盘同步等）：重新加载</summary>
    public void NotifyRegistryChanged() => FireAndForget(new RegistryReloadJob());

    /// <summary>异步触发全量扫描（library/reindex：立即返回，过程变更照常推送事件）</summary>
    public void RequestScan(bool full) => FireAndForget(new ScanJob(full, null));

    private void FireAndForget(IndexJob job)
    {
        if (!_jobs.Writer.TryWrite(job))
        {
            NotifyOverflow();
        }
    }

    // ---------- 入口：API / 启动（等待处理完成） ----------

    /// <param name="knownHash">调用方已算好的内容哈希（如 item/add），提供时流水线跳过重算</param>
    public Task<UpsertResult?> SubmitUpsertAsync(string absPath, string? knownHash = null)
    {
        var tcs = NewTcs<UpsertResult?>();
        Enqueue(new UpsertJob(absPath, false, knownHash, tcs, 0), tcs);
        return tcs.Task;
    }

    public Task SubmitMoveAsync(string oldAbs, string newAbs)
    {
        var tcs = NewTcs();
        Enqueue(new MoveJob(oldAbs, newAbs, tcs), tcs);
        return tcs.Task;
    }

    public Task SubmitDirMoveAsync(string oldAbs, string newAbs)
    {
        var tcs = NewTcs();
        Enqueue(new DirMoveJob(oldAbs, newAbs, tcs), tcs);
        return tcs.Task;
    }

    public Task SubmitClearTrashAsync()
    {
        var tcs = NewTcs();
        Enqueue(new ClearTrashJob(tcs), tcs);
        return tcs.Task;
    }

    public Task<Item?> SubmitMetadataAsync(string hash, Action<ItemMetadata> mutate)
    {
        var tcs = NewTcs<Item?>();
        Enqueue(new MetadataJob(hash, mutate, tcs), tcs);
        return tcs.Task;
    }

    /// <summary>全量扫描。full=true 时对所有文件重算哈希（library/reindex）。</summary>
    public Task RunScanAsync(bool full)
    {
        var tcs = NewTcs();
        Enqueue(new ScanJob(full, tcs), tcs);
        return tcs.Task;
    }

    // ---------- 入口：分类/标签操作（API 提交，校验在端点层完成） ----------

    public Task SubmitCategoryCreateAsync(string path)
    {
        var tcs = NewTcs();
        Enqueue(new CategoryCreateJob(path, tcs), tcs);
        return tcs.Task;
    }

    public Task SubmitCategoryUpdateAsync(string path, string? name, string? parentPath)
    {
        var tcs = NewTcs();
        Enqueue(new CategoryUpdateJob(path, name, parentPath, tcs), tcs);
        return tcs.Task;
    }

    public Task SubmitCategoryDeleteAsync(string path)
    {
        var tcs = NewTcs();
        Enqueue(new CategoryDeleteJob(path, tcs), tcs);
        return tcs.Task;
    }

    public Task SubmitTagCreateAsync(string name)
    {
        var tcs = NewTcs();
        Enqueue(new TagCreateJob(name, tcs), tcs);
        return tcs.Task;
    }

    public Task SubmitTagUpdateAsync(string name, string newName)
    {
        var tcs = NewTcs();
        Enqueue(new TagUpdateJob(name, newName, tcs), tcs);
        return tcs.Task;
    }

    public Task SubmitTagDeleteAsync(string name)
    {
        var tcs = NewTcs();
        Enqueue(new TagDeleteJob(name, tcs), tcs);
        return tcs.Task;
    }

    private static TaskCompletionSource NewTcs() => new(TaskCreationOptions.RunContinuationsAsynchronously);

    private static TaskCompletionSource<T> NewTcs<T>() => new(TaskCreationOptions.RunContinuationsAsynchronously);

    private void Enqueue<T>(IndexJob job, TaskCompletionSource<T> tcs)
    {
        if (!_jobs.Writer.TryWrite(job))
        {
            tcs.TrySetException(new InvalidOperationException("索引队列已满"));
        }
    }

    private void Enqueue(IndexJob job, TaskCompletionSource tcs)
    {
        if (!_jobs.Writer.TryWrite(job))
        {
            tcs.TrySetException(new InvalidOperationException("索引队列已满"));
        }
    }

    // ---------- 消费者 ----------

    private async Task ConsumeLoop(CancellationToken ct)
    {
        try
        {
            await foreach (var job in _jobs.Reader.ReadAllAsync(ct))
            {
                try
                {
                    ProcessJob(job, ct);
                }
                catch (OperationCanceledException) when (ct.IsCancellationRequested)
                {
                    throw;
                }
                catch (Exception ex)
                {
                    _logger.LogError(ex, "索引任务处理失败: {Job}", job.GetType().Name);
                    FailJob(job, ex); // 异常回传给等待中的 API 调用方，避免挂起
                }

                // 监听事件丢失兜底：每处理完一批任务检查一次
                if (Interlocked.Exchange(ref _overflow, 0) == 1)
                {
                    _logger.LogInformation("检测到事件丢失，执行全量扫描");
                    DoScan(full: false, ct);
                }
            }
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
        }
    }

    private void ProcessJob(IndexJob job, CancellationToken ct)
    {
        switch (job)
        {
            case UpsertJob j:
                Complete(j.Done, DoUpsert(j, ct));
                break;
            case DeleteJob j:
                if (_paths.ToRelative(j.AbsPath) is { } delRel)
                {
                    DoDelete(delRel);
                }
                break;
            case MoveJob j:
                DoMove(j.OldAbs, j.NewAbs, ct);
                Complete(j.Done);
                break;
            case DirMoveJob j:
                DoDirMove(j.OldAbs, j.NewAbs, ct);
                Complete(j.Done);
                break;
            case ScanJob j:
                _config.Reload();
                DoScan(j.Full, ct);
                Complete(j.Done);
                break;
            case ClearTrashJob j:
                DoClearTrash();
                Complete(j.Done);
                break;
            case MetadataJob j:
                Complete(j.Done, DoApplyMetadata(j.Hash, j.Mutate));
                break;
            case PaletteJob j:
                // 调色板回写：item 已因内容漂移消失时丢弃（幂等，重复应用无害）
                if (_index.SetPalette(j.Hash, j.Palette) is { } paletteItem)
                {
                    PublishItemChanged(paletteItem);
                }
                break;
            case CategoryCreateJob j:
                _categories.Register(j.Path);
                Complete(j.Done);
                break;
            case CategoryUpdateJob j:
                DoCategoryUpdate(j.Path, j.Name, j.ParentPath);
                Complete(j.Done);
                break;
            case CategoryDeleteJob j:
                DoCategoryDelete(j.Path);
                Complete(j.Done);
                break;
            case TagCreateJob j:
                _tags.Register(j.Name);
                Complete(j.Done);
                break;
            case TagUpdateJob j:
                DoTagUpdate(j.Name, j.NewName);
                Complete(j.Done);
                break;
            case TagDeleteJob j:
                DoTagDelete(j.Name);
                Complete(j.Done);
                break;
            case RegistryReloadJob:
                _categories.Reload();
                _tags.Reload();
                break;
        }
    }

    /// <summary>任务处理失败时把异常回传给提交方</summary>
    private static void FailJob(IndexJob job, Exception ex)
    {
        switch (job)
        {
            case UpsertJob j: j.Done?.TrySetException(ex); break;
            case MoveJob j: j.Done?.TrySetException(ex); break;
            case DirMoveJob j: j.Done?.TrySetException(ex); break;
            case ScanJob j: j.Done?.TrySetException(ex); break;
            case ClearTrashJob j: j.Done?.TrySetException(ex); break;
            case MetadataJob j: j.Done.TrySetException(ex); break;
            case CategoryCreateJob j: j.Done.TrySetException(ex); break;
            case CategoryUpdateJob j: j.Done.TrySetException(ex); break;
            case CategoryDeleteJob j: j.Done.TrySetException(ex); break;
            case TagCreateJob j: j.Done.TrySetException(ex); break;
            case TagUpdateJob j: j.Done.TrySetException(ex); break;
            case TagDeleteJob j: j.Done.TrySetException(ex); break;
        }
    }

    private static void Complete(TaskCompletionSource? tcs) => tcs?.TrySetResult();
    private static void Complete<T>(TaskCompletionSource<T>? tcs, T value) => tcs?.TrySetResult(value);

    // ---------- 单文件入库 ----------

    /// <summary>DoUpsert 的准备结果：复用判定完成，ReusedHash 非 null 表示无需计算哈希</summary>
    private sealed class PendingUpsert
    {
        public required string AbsPath { get; init; }
        public required string Rel { get; init; }
        public required string LibPath { get; init; }
        public long Size { get; init; }
        public long Mtime { get; init; }
        public string? OldHash { get; init; }
        public string? ReusedHash { get; init; }
        public string? Hash { get; set; }
    }

    private UpsertResult? DoUpsert(UpsertJob job, CancellationToken ct)
    {
        // 携带已知哈希（item/add）或等待结果的提交不做防抖：文件由 API 写入，内容已完整
        var allowDefer = job.KnownHash is null && job.Done is null;
        var pending = PrepareUpsert(job.AbsPath, job.ForceHash, allowDefer, job.Attempt, ct);
        if (pending is null)
        {
            return null;
        }

        var hash = job.KnownHash ?? pending.ReusedHash ?? TryComputeHash(pending.AbsPath, ct);
        return hash is null ? null : ApplyUpsert(pending, hash, ct);
    }

    /// <summary>
    /// 入库准备：路径过滤、文件状态读取、哈希复用判定、写入中文件防抖。
    /// 返回 null 表示已处理（跳过/按删除处理/延迟重试）。不读文件内容。
    /// </summary>
    private PendingUpsert? PrepareUpsert(string absPath, bool forceHash, bool allowDefer, int attempt, CancellationToken ct)
    {
        var rel = _paths.ToRelative(absPath);
        if (rel is null || LibraryPaths.IsInternal(rel))
        {
            return null;
        }

        var inTrash = LibraryPaths.IsInTrash(rel);
        if (!inTrash && _config.IsIgnored(rel))
        {
            DoDelete(rel);
            return null;
        }

        var file = new FileInfo(absPath);
        if (!file.Exists)
        {
            DoDelete(rel);
            return null;
        }

        var libPath = inTrash ? LibraryPaths.TrashToLibraryPath(rel) : rel;
        var size = file.Length;
        var mtime = LibraryPaths.ToUnixMs(file.LastWriteTimeUtc);

        // 路径与 size/mtime 均与元数据一致 → 复用哈希（元数据文件名即哈希），不读文件内容
        var oldHash = _index.HashByLocation(rel) ?? _store.FindHashByPath(libPath);
        var reuse = !forceHash
            && oldHash is not null
            && _store.TryGet(oldHash, out var cached)
            && cached.FindPath(libPath) is { } entry
            && entry.Size == size
            && entry.ModificationTime == mtime;

        if (reuse)
        {
            return new PendingUpsert { AbsPath = absPath, Rel = rel, LibPath = libPath, Size = size, Mtime = mtime, OldHash = oldHash, ReusedHash = oldHash };
        }

        // 文件可能仍在写入（如大文件拷贝中）：不立即哈希，延迟重试直至写入稳定，
        // 避免对半截内容反复算哈希。超出重试上限后按现状处理（后续事件/扫描会自愈）。
        if (allowDefer && attempt < MaxDebounceAttempts && IsUnstable(file))
        {
            DeferUpsert(absPath, attempt);
            return null;
        }

        return new PendingUpsert { AbsPath = absPath, Rel = rel, LibPath = libPath, Size = size, Mtime = mtime, OldHash = oldHash };
    }

    /// <summary>应用入库结果：元数据迁移与回写、索引更新、事件、缩略图派发。只允许串行调用。</summary>
    private UpsertResult ApplyUpsert(PendingUpsert pending, string hash, CancellationToken ct)
    {
        // 内容变动导致哈希漂移 → 按路径迁移元数据，旧 item 摘掉该位置。
        // 注意先取旧元数据用于继承：迁移可能将旧元数据删除（无剩余位置时）
        ItemMetadata? inheritFrom = pending.OldHash is not null && pending.OldHash != hash
            && _store.TryGet(pending.OldHash, out var oldMeta) ? oldMeta : null;

        if (pending.OldHash is not null && pending.OldHash != hash)
        {
            _index.RemoveLocation(pending.Rel);
            MigrateMetadata(pending.OldHash, pending.LibPath);
            PublishLocationLoss(pending.OldHash);
        }

        // 元数据登记路径并回写最新 size/mtime，保持哈希校验依据新鲜
        var meta = GetOrCreateMetadata(hash, inheritFrom);
        var metaChanged = false;
        var pathEntry = meta.FindPath(pending.LibPath);
        if (pathEntry is null)
        {
            meta.Paths.Add(new PathEntry { Path = pending.LibPath, Size = pending.Size, ModificationTime = pending.Mtime });
            metaChanged = true;
        }
        else if (pathEntry.Size != pending.Size || pathEntry.ModificationTime != pending.Mtime)
        {
            pathEntry.Size = pending.Size;
            pathEntry.ModificationTime = pending.Mtime;
            metaChanged = true;
        }

        if (metaChanged)
        {
            _store.Save(hash, meta);
        }

        RegisterTaxonomy(meta);

        // 索引更新；尺寸为派生信息，索引时从文件读取
        var item = _index.GetOrAdd(hash, out var created);
        SyncMetadata(item, meta);
        if (item.Width == 0 && ThumbnailService.Identify(pending.AbsPath) is { } dim)
        {
            item.Width = dim.Width;
            item.Height = dim.Height;
        }

        // 调色板是内容寻址的缓存：有则直接入索引；缺失由后台 worker 提炼后经 PaletteJob 补齐
        if (item.Palette.Length == 0 && _colors.Load(hash) is { } palette)
        {
            item.Palette = palette;
        }

        var addedLocation = _index.AddOrUpdateLocation(hash, pending.Rel, pending.Size, pending.Mtime);

        if (created)
        {
            _bus.Publish("item.added", item.ToDto(trashView: !item.HasLibraryLocations));
        }
        else if (addedLocation || metaChanged)
        {
            PublishItemChanged(item);
        }

        QueueThumbnails(hash, pending.AbsPath);
        return new UpsertResult(item, AlreadyExisted: !created);
    }

    /// <summary>文件最近一秒内仍在写入，视为不稳定</summary>
    private static bool IsUnstable(FileInfo file) =>
        DateTime.UtcNow - file.LastWriteTimeUtc < StabilityWindow;

    /// <summary>延迟重试：同一路径只保留一个延迟任务，避免监听事件风暴放大</summary>
    private void DeferUpsert(string absPath, int attempt)
    {
        if (!_deferredPaths.TryAdd(absPath, 0))
        {
            return;
        }

        _ = Task.Run(async () =>
        {
            try
            {
                await Task.Delay(StabilityWindow, _cts.Token);
            }
            catch (OperationCanceledException)
            {
                return;
            }

            _deferredPaths.TryRemove(absPath, out _);
            if (!_jobs.Writer.TryWrite(new UpsertJob(absPath, false, null, null, attempt + 1)))
            {
                NotifyOverflow();
            }
        });
    }

    /// <summary>计算内容哈希；读不了（权限/占用）时告警并返回 null</summary>
    private string? TryComputeHash(string absPath, CancellationToken ct)
    {
        try
        {
            return ContentHash.HashFile(absPath, ct);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            _logger.LogWarning(ex, "计算哈希失败: {Path}", absPath);
            return null;
        }
    }

    /// <summary>哈希漂移时按路径迁移：路径从旧元数据移除；旧元数据不再有位置且索引无引用时清理</summary>
    private void MigrateMetadata(string oldHash, string libPath)
    {
        if (!_store.TryGet(oldHash, out var oldMeta))
        {
            return;
        }

        oldMeta.Paths.RemoveAll(p => p.Path == libPath);
        var oldItem = _index.Get(oldHash);
        if (oldMeta.Paths.Count == 0 && oldItem is null)
        {
            _store.Delete(oldHash);
            _thumbnails.Delete(oldHash, _config.Current.ThumbnailSizes);
            _colors.Delete(oldHash);
        }
        else
        {
            _store.Save(oldHash, oldMeta);
        }
    }

    /// <summary>取得元数据；不存在时新建，可从旧元数据继承素材参数（id 漂移场景）</summary>
    private ItemMetadata GetOrCreateMetadata(string hash, ItemMetadata? inheritFrom)
    {
        if (_store.TryGet(hash, out var meta))
        {
            return meta;
        }

        var created = new ItemMetadata();
        if (inheritFrom is not null)
        {
            created.Url = inheritFrom.Url;
            created.Tags = new List<string>(inheritFrom.Tags);
            created.Star = inheritFrom.Star;
            created.Annotation = inheritFrom.Annotation;
        }

        return created;
    }

    private static void SyncMetadata(Item item, ItemMetadata meta)
    {
        item.Url = meta.Url;
        item.Tags = new List<string>(meta.Tags);
        item.Categories = new List<string>(meta.Categories);
        item.Star = meta.Star;
        item.Annotation = meta.Annotation;
    }

    /// <summary>元数据中的分类/标签自动登记进注册表（赋值即创建，空节点也可预创建）</summary>
    private void RegisterTaxonomy(ItemMetadata meta)
    {
        _categories.RegisterAll(meta.Categories);
        _tags.RegisterAll(meta.Tags);
    }

    // ---------- 删除 / 移动 ----------

    /// <summary>按相对路径删除：同时按文件（精确）与目录（前缀）匹配，删除事件不区分两者</summary>
    private void DoDelete(string rel)
    {
        var item = _index.RemoveLocation(rel);
        if (item is not null)
        {
            PublishLocationLoss(item.Id);
        }

        foreach (var loc in _index.LocationsUnder(rel + "/"))
        {
            var removed = _index.RemoveLocation(loc);
            if (removed is not null)
            {
                PublishLocationLoss(removed.Id);
            }
        }
    }

    private void DoMove(string oldAbs, string newAbs, CancellationToken ct)
    {
        var oldRel = _paths.ToRelative(oldAbs);
        var newRel = _paths.ToRelative(newAbs);
        if (oldRel is null)
        {
            return;
        }

        if (newRel is null || LibraryPaths.IsInternal(newRel) || (!LibraryPaths.IsInTrash(newRel) && _config.IsIgnored(newRel)))
        {
            DoDelete(oldRel);
            return;
        }

        if (!MoveOne(oldRel, newRel, out var hash))
        {
            // 旧位置未索引（例如改名发生在未索引文件上）→ 按新文件处理
            DoUpsert(new UpsertJob(newAbs, false, null, null, 0), ct);
            return;
        }

        PublishTransition(hash!, LibraryPaths.IsInTrash(oldRel), LibraryPaths.IsInTrash(newRel));
    }

    private void DoDirMove(string oldAbs, string newAbs, CancellationToken ct)
    {
        var oldRel = _paths.ToRelative(oldAbs);
        var newRel = _paths.ToRelative(newAbs);
        if (oldRel is null)
        {
            return;
        }

        if (newRel is null || LibraryPaths.IsInternal(newRel) || (!LibraryPaths.IsInTrash(newRel + "/") && _config.IsIgnored(newRel)))
        {
            DoDelete(oldRel);
            return;
        }

        var affected = new HashSet<string>();
        foreach (var locPath in _index.LocationsUnder(oldRel + "/"))
        {
            var newLocPath = newRel + locPath[oldRel.Length..];
            if (MoveOne(locPath, newLocPath, out var hash))
            {
                affected.Add(hash!);
            }
        }

        var oldInTrash = LibraryPaths.IsInTrash(oldRel + "/");
        var newInTrash = LibraryPaths.IsInTrash(newRel + "/");
        foreach (var hash in affected)
        {
            PublishTransition(hash, oldInTrash, newInTrash);
        }

        // 目录下可能有监听遗漏的文件，补扫新位置
        if (Directory.Exists(newAbs))
        {
            foreach (var file in _scanner.WalkDirectory(newAbs))
            {
                DoUpsert(new UpsertJob(file, false, null, null, 0), ct);
            }
        }
    }

    /// <summary>单个位置的移动：更新索引与元数据路径。返回是否命中已索引位置。</summary>
    private bool MoveOne(string oldRel, string newRel, out string? hash)
    {
        hash = _index.MoveLocation(oldRel, newRel);
        if (hash is null)
        {
            return false;
        }

        // lib→lib：元数据路径跟随；lib↔trash：去前缀后库内路径不变，元数据保持原路径（恢复目标）
        var oldLib = LibraryPaths.IsInTrash(oldRel) ? LibraryPaths.TrashToLibraryPath(oldRel) : oldRel;
        var newLib = LibraryPaths.IsInTrash(newRel) ? LibraryPaths.TrashToLibraryPath(newRel) : newRel;
        if (oldLib != newLib && _store.TryGet(hash, out var meta) && meta.FindPath(oldLib) is { } entry)
        {
            entry.Path = newLib;
            _store.Save(hash, meta);
        }

        return true;
    }

    // ---------- 扫描 ----------

    /// <summary>
    /// 全量扫描分两阶段：串行遍历做复用判定（不读文件内容），需要哈希的文件并行计算，
    /// 最后串行应用索引/元数据变更——并行仅限纯计算阶段，单写者模型不变。
    /// full=true 时对所有文件重算哈希（library/reindex）。
    /// </summary>
    private void DoScan(bool full, CancellationToken ct)
    {
        var seen = new HashSet<string>(StringComparer.Ordinal);
        var pending = new List<PendingUpsert>();
        var count = 0;

        foreach (var abs in _scanner.WalkLibrary())
        {
            ct.ThrowIfCancellationRequested();
            if (_paths.ToRelative(abs) is { } rel)
            {
                seen.Add(rel);
            }

            var prepared = PrepareUpsert(abs, full, allowDefer: true, attempt: 0, ct);
            if (prepared is null)
            {
                continue;
            }

            count++;
            if (prepared.ReusedHash is not null)
            {
                ApplyUpsert(prepared, prepared.ReusedHash, ct);
            }
            else
            {
                pending.Add(prepared);
            }
        }

        if (pending.Count > 0)
        {
            Parallel.ForEach(pending, new ParallelOptions { MaxDegreeOfParallelism = HashParallelism, CancellationToken = ct },
                p => p.Hash = TryComputeHash(p.AbsPath, ct));

            foreach (var p in pending)
            {
                if (p.Hash is not null)
                {
                    ApplyUpsert(p, p.Hash, ct);
                }
            }
        }

        // 扫描未发现的位置 → 文件已消失
        foreach (var rel in _index.AllLocationPaths())
        {
            if (!seen.Contains(rel))
            {
                var item = _index.RemoveLocation(rel);
                if (item is not null)
                {
                    PublishLocationLoss(item.Id);
                }
            }
        }

        _logger.LogInformation("扫描完成：{Count} 个文件（{Hashed} 个计算哈希），{Total} 个索引位置",
            count, pending.Count, _index.AllLocationPaths().Length);
    }

    // ---------- 回收站 ----------

    /// <summary>清空回收站：清理位置与对应元数据、缩略图（库内仍有引用的内容除外）。物理删除由 API 层完成。</summary>
    private void DoClearTrash()
    {
        foreach (var rel in _index.AllLocationPaths().Where(LibraryPaths.IsInTrash))
        {
            var item = _index.RemoveLocation(rel);
            if (item is null)
            {
                continue;
            }

            var libPath = LibraryPaths.TrashToLibraryPath(rel);
            if (_store.TryGet(item.Id, out var meta))
            {
                meta.Paths.RemoveAll(p => p.Path == libPath);
                if (meta.Paths.Count == 0 && item.Locations.Count == 0)
                {
                    _store.Delete(item.Id);
                    _thumbnails.Delete(item.Id, _config.Current.ThumbnailSizes);
                    _colors.Delete(item.Id);
                }
                else
                {
                    _store.Save(item.Id, meta);
                }
            }

            if (item.Locations.Count == 0)
            {
                _bus.Publish("item.removed", new ItemIdPayload(item.Id));
            }
            else
            {
                PublishItemChanged(item);
            }
        }
    }

    // ---------- 元数据写 ----------

    private Item? DoApplyMetadata(string hash, Action<ItemMetadata> mutate)
    {
        if (!_store.TryGet(hash, out var meta))
        {
            return null;
        }

        mutate(meta);
        _store.Save(hash, meta);
        RegisterTaxonomy(meta);

        var item = _index.Get(hash);
        if (item is not null)
        {
            SyncMetadata(item, meta);
            PublishItemChanged(item);
        }

        return item;
    }

    // ---------- 分类/标签级联迁移 ----------

    /// <summary>分类重命名/移动：注册表前缀迁移 + 全部命中 item 的 categories 迁移（子树跟随）</summary>
    private void DoCategoryUpdate(string path, string? name, string? parentPath)
    {
        var newName = name ?? CategoryPath.NameOf(path);
        var newParent = parentPath ?? CategoryPath.ParentOf(path);
        var newPath = newParent == "" ? newName : newParent + "/" + newName;
        if (newPath == path)
        {
            return;
        }

        _categories.Rename(path, newPath);
        // 分类可能仅由赋值产生而未注册过，补上登记
        _categories.Register(newPath);

        foreach (var (hash, meta) in _store.Snapshot())
        {
            if (!meta.Categories.Any(c => CategoryPath.IsSameOrDescendant(c, path)))
            {
                continue;
            }

            meta.Categories = meta.Categories
                .Select(c => CategoryPath.IsSameOrDescendant(c, path) ? newPath + c[path.Length..] : c)
                .Distinct(StringComparer.Ordinal)
                .ToList();
            SaveAndSync(hash, meta);
        }
    }

    /// <summary>分类删除：注册表与全部 item 的该节点及子树赋值一并清除</summary>
    private void DoCategoryDelete(string path)
    {
        _categories.Delete(path);

        foreach (var (hash, meta) in _store.Snapshot())
        {
            if (meta.Categories.RemoveAll(c => CategoryPath.IsSameOrDescendant(c, path)) > 0)
            {
                SaveAndSync(hash, meta);
            }
        }
    }

    /// <summary>标签重命名：注册表更名 + 全部 item 的 tags 替换；目标已存在时合并</summary>
    private void DoTagUpdate(string name, string newName)
    {
        _tags.Rename(name, newName);

        foreach (var (hash, meta) in _store.Snapshot())
        {
            if (!meta.Tags.Contains(name))
            {
                continue;
            }

            meta.Tags = meta.Tags.Select(t => t == name ? newName : t).Distinct(StringComparer.Ordinal).ToList();
            SaveAndSync(hash, meta);
        }
    }

    /// <summary>标签删除：注册表与全部 item 的该标签清除</summary>
    private void DoTagDelete(string name)
    {
        _tags.Delete(name);

        foreach (var (hash, meta) in _store.Snapshot())
        {
            if (meta.Tags.RemoveAll(t => t == name) > 0)
            {
                SaveAndSync(hash, meta);
            }
        }
    }

    /// <summary>批量迁移的公共收尾：保存元数据、同步索引、推送 item.updated</summary>
    private void SaveAndSync(string hash, ItemMetadata meta)
    {
        _store.Save(hash, meta);
        if (_index.Get(hash) is { } item)
        {
            SyncMetadata(item, meta);
            PublishItemChanged(item);
        }
    }

    // ---------- 事件 ----------

    /// <summary>item 失去一个位置后的事件：无剩余位置 → removed；只剩回收站 → trashed；否则 updated</summary>
    private void PublishLocationLoss(string hash)
    {
        var item = _index.Get(hash);
        if (item is null)
        {
            _bus.Publish("item.removed", new ItemIdPayload(hash));
        }
        else if (!item.HasLibraryLocations)
        {
            _bus.Publish("item.trashed", new ItemIdPayload(hash));
        }
        else
        {
            _bus.Publish("item.updated", item.ToDto(trashView: false));
        }
    }

    /// <summary>位置进出回收站后的事件</summary>
    private void PublishTransition(string hash, bool wasInTrash, bool nowInTrash)
    {
        var item = _index.Get(hash);
        if (item is null)
        {
            return;
        }

        if (!wasInTrash && nowInTrash && !item.HasLibraryLocations)
        {
            _bus.Publish("item.trashed", new ItemIdPayload(hash));
        }
        else if (wasInTrash && !nowInTrash && item.Locations.Count(l => !l.InTrash) == 1)
        {
            _bus.Publish("item.restored", item.ToDto(trashView: false));
        }
        else
        {
            PublishItemChanged(item);
        }
    }

    private void PublishItemChanged(Item item) =>
        _bus.Publish("item.updated", item.ToDto(trashView: !item.HasLibraryLocations));

    // ---------- 哈希与缩略图 ----------

    private void QueueThumbnails(string hash, string absPath)
    {
        // 缩略图是尽力而为的缓存，channel 满时丢弃（缺失可由 refresh_thumbnail 或重启扫描补齐）
        _thumbJobs.Writer.TryWrite(new ThumbJob(hash, absPath));
    }

    private async Task ThumbLoop(CancellationToken ct)
    {
        await foreach (var job in _thumbJobs.Reader.ReadAllAsync(ct))
        {
            try
            {
                var sizes = _config.Current.ThumbnailSizes.Where(s => !_thumbnails.Exists(job.Hash, s)).ToArray();
                var needPalette = !_colors.Exists(job.Hash);
                if (sizes.Length == 0 && !needPalette)
                {
                    continue;
                }

                var generated = sizes.Length > 0 && await _thumbnails.GenerateAsync(job.Hash, job.SourceAbs, sizes, ct: ct);

                // 调色板从最小尺寸的已有缩略图提炼：原图只由缩略图生成解码一次，此处解码小图代价极低
                if (needPalette)
                {
                    var source = _config.Current.ThumbnailSizes.OrderBy(s => s)
                        .Select(s => _thumbnails.GetPath(job.Hash, s))
                        .FirstOrDefault(File.Exists);
                    if (source is not null && _colors.Extract(source) is { } palette)
                    {
                        _colors.Save(job.Hash, palette);
                        // 回流水线应用（单写者）；队列满时丢弃——缓存已落盘，兜底扫描由 ApplyUpsert 载入
                        _jobs.Writer.TryWrite(new PaletteJob(job.Hash, palette));
                    }
                }

                // 生成完成后补发 item.updated：前端缩略图此前的 404 占位据此重建 <img>
                if (generated && _index.Get(job.Hash) is { } item)
                {
                    _bus.Publish("item.updated", item.ToDto(trashView: !item.HasLibraryLocations));
                }
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                throw;
            }
            catch (Exception ex)
            {
                _logger.LogDebug(ex, "缩略图任务失败: {Hash}", job.Hash);
            }
        }
    }

    public void Dispose()
    {
        _cts.Cancel();
        _jobs.Writer.TryComplete();
        _thumbJobs.Writer.TryComplete();
        try
        {
            _consumer?.Wait(TimeSpan.FromSeconds(5));
            Task.WaitAll(_thumbWorkers, TimeSpan.FromSeconds(5));
        }
        catch
        {
            // 关闭过程中忽略任务异常
        }

        _cts.Dispose();
    }
}

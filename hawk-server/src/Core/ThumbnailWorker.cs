using System.Threading.Channels;

namespace Hawk.Server.Core;

/// <summary>
/// 缩略图/调色板后台 worker:独立队列与线程池(CPU 密集),不阻塞索引消费循环。
/// 从 IndexPipeline 拆出;完成结果经回调回流水线(PaletteJob 回写索引,保持单写者)。
/// 队列是尽力而为的缓存:满时丢弃,缺失可由 refresh_thumbnail 或重启扫描补齐。
/// </summary>
public sealed class ThumbnailWorker : IDisposable
{
    private sealed record ThumbJob(string Hash, string SourceAbs);

    private readonly ThumbnailService _thumbnails;
    private readonly ColorService _colors;
    private readonly LibraryConfig _config;
    private readonly EventBus _bus;
    private readonly ILogger<ThumbnailWorker> _logger;

    /// <summary>生成完成后取 item 投影(补发 item.updated);返回 null 表示 item 已消失,跳过</summary>
    private Func<string, ItemDto?> _getItemDto = _ => null;

    /// <summary>调色板提炼完成后回写流水线(单写者);队列满时由调用方丢弃</summary>
    private Action<string, PaletteColor[]> _enqueuePalette = (_, _) => { };

    private readonly Channel<ThumbJob> _jobs = Channel.CreateBounded<ThumbJob>(
        new BoundedChannelOptions(4096) { FullMode = BoundedChannelFullMode.Wait, SingleReader = false });

    private readonly CancellationTokenSource _cts = new();
    private Task[] _workers = [];

    private int _queued;
    private int _active;
    private long _lastProgressAt;
    private bool _progressIdle = true;

    public ThumbnailWorker(
        ThumbnailService thumbnails,
        ColorService colors,
        LibraryConfig config,
        EventBus bus,
        ILogger<ThumbnailWorker> logger)
    {
        _thumbnails = thumbnails;
        _colors = colors;
        _config = config;
        _bus = bus;
        _logger = logger;
    }

    /// <summary>由 IndexPipeline 装配:索引访问与调色板回写的闭环在流水线侧(单写者)</summary>
    public void Attach(Func<string, ItemDto?> getItemDto, Action<string, PaletteColor[]> enqueuePalette)
    {
        _getItemDto = getItemDto;
        _enqueuePalette = enqueuePalette;
    }

    /// <summary>积压快照(排队 + 生成中);task.progress 事件与 app/status 端点共用</summary>
    public (int Pending, int Active) Backlog =>
        (System.Threading.Volatile.Read(ref _queued), System.Threading.Volatile.Read(ref _active));

    public void Start()
    {
        // 纯 CPU 后台任务(解码/缩放/WebP 编码),与索引互不阻塞;
        // 并发度与扫描哈希一致(CPU/2、封顶 16)——并发文件数就是并行核数,过低则 CPU 吃不满
        _workers = Enumerable.Range(0, Math.Clamp(Environment.ProcessorCount / 2, 2, 16))
            .Select(_ => WorkLoop(_cts.Token))
            .ToArray();
    }

    /// <summary>派发缩略图任务(尽力而为:channel 满时丢弃)</summary>
    public void Enqueue(string hash, string sourceAbs)
    {
        if (_jobs.Writer.TryWrite(new ThumbJob(hash, sourceAbs)))
        {
            Interlocked.Increment(ref _queued);
        }
    }

    private async Task WorkLoop(CancellationToken ct)
    {
        await foreach (var job in _jobs.Reader.ReadAllAsync(ct))
        {
            Interlocked.Decrement(ref _queued);
            Interlocked.Increment(ref _active);
            try
            {
                var sizes = _config.Current.ThumbnailSizes.Where(s => !_thumbnails.Exists(job.Hash, s)).ToArray();
                var needPalette = !_colors.Exists(job.Hash);
                if (sizes.Length == 0 && !needPalette)
                {
                    continue;
                }

                var generated = sizes.Length > 0 && await _thumbnails.GenerateAsync(job.Hash, job.SourceAbs, sizes, ct: ct);

                // 调色板从最小尺寸的已有缩略图提炼:原图只由缩略图生成解码一次,此处解码小图代价极低
                if (needPalette)
                {
                    var source = _config.Current.ThumbnailSizes.OrderBy(s => s)
                        .Select(s => _thumbnails.GetPath(job.Hash, s))
                        .FirstOrDefault(File.Exists);
                    if (source is not null && _colors.Extract(source) is { } palette)
                    {
                        _colors.Save(job.Hash, palette);
                        // 回流水线应用(单写者);队列满时丢弃——缓存已落盘,兜底扫描由 ApplyUpsert 载入
                        _enqueuePalette(job.Hash, palette);
                    }
                }

                // 生成完成后补发 item.updated:前端缩略图此前的 404 占位据此重建 <img>
                if (generated && _getItemDto(job.Hash) is { } dto)
                {
                    _bus.Publish(ItemEvents.Updated, dto);
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
            finally
            {
                Interlocked.Decrement(ref _active);
                ReportProgress();
            }
        }
    }

    /// <summary>
    /// 积压变化时节流推送 task.progress(500ms 一帧);刚从非空闲转空闲时补发一帧,让客户端撤掉进度指示。
    /// SSE 断开的客户端可用 GET /api/v1/app/status 轮询同一快照。
    /// </summary>
    private void ReportProgress()
    {
        var (pending, active) = Backlog;
        var idle = pending == 0 && active == 0;
        var now = DateTime.UtcNow.Ticks;
        var last = Interlocked.Read(ref _lastProgressAt);
        var due = now - last >= TimeSpan.TicksPerMillisecond * 500;
        if (!due && !(idle && !_progressIdle))
        {
            return;
        }

        if (Interlocked.CompareExchange(ref _lastProgressAt, now, last) != last)
        {
            return;
        }

        _progressIdle = idle;
        _bus.Publish(ItemEvents.TaskProgress, new TaskProgress("thumbnail", pending, active));
    }

    public void Dispose()
    {
        _cts.Cancel();
        _jobs.Writer.TryComplete();
        try
        {
            Task.WaitAll(_workers, TimeSpan.FromSeconds(5));
        }
        catch
        {
            // 关闭过程中忽略任务异常
        }

        _cts.Dispose();
    }
}

/// <summary>后台任务进度快照(task.progress 事件与 app/status 端点共用)</summary>
public sealed record TaskProgress(string Task, int Pending, int Active);

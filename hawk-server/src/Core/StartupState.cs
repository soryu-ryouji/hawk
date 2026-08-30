namespace Hawk.Server.Core;

/// <summary>
/// 启动状态：Kestrel 先监听、初始索引后台构建（先监听端口、扫描后台进行的启动模型）。
/// 就绪前 /health 返回 503、/api/* 被 ReadyGateMiddleware 拦截（app/startup 除外）；
/// 进度经 /api/v1/app/startup 查询，不再走 stdout 私有协议。
/// </summary>
public sealed class StartupState
{
    /// <summary>当前阶段：scan / hash / apply / sync（元数据对账，仅启动期）/ done（就绪后为 done）</summary>
    public string Phase { get; private set; } = "scan";

    public int Processed { get; private set; }
    public int Total { get; private set; }

    /// <summary>初始索引是否完成（此后 /health 200、API 网关放行）</summary>
    public bool IsReady { get; private set; }

    /// <summary>初始索引失败原因（null 表示未失败）；失败后常驻，需人工处理（如修复目录后重启）</summary>
    public string? Error { get; private set; }

    public void Report(IndexPipeline.ScanProgress progress)
    {
        Phase = progress.Phase;
        Processed = progress.Processed;
        Total = progress.Total;
    }

    public void MarkReady() => IsReady = true;

    public void Fail(Exception exception) => Error = exception.Message;
}

namespace Hawk.Server.Core;

/// <summary>
/// FileSystemWatcher 封装。事件统一回调给索引流水线；
/// .hawk/ 内部（回收站除外）与 config.toml 之外的 hawk 自身文件不产生索引事件。
/// </summary>
public sealed class LibraryWatcher : IDisposable
{
    private readonly LibraryPaths _paths;
    private readonly ILogger<LibraryWatcher> _logger;
    private FileSystemWatcher? _watcher;

    /// <summary>文件创建/内容变更（绝对路径）</summary>
    public event Action<string>? FileUpsert;

    /// <summary>文件或目录删除（绝对路径；流水线按路径与目录前缀双重匹配处理）</summary>
    public event Action<string>? Deleted;

    /// <summary>移动/重命名（旧绝对路径，新绝对路径）</summary>
    public event Action<string, string>? Moved;

    /// <summary>config.toml 变更</summary>
    public event Action? ConfigChanged;

    /// <summary>categories.toml / tags.toml 注册表变更（含外部同步写入）</summary>
    public event Action? RegistryChanged;

    /// <summary>事件缓冲溢出，需要全量扫描兜底</summary>
    public event Action? Overflowed;

    public LibraryWatcher(LibraryPaths paths, ILogger<LibraryWatcher> logger)
    {
        _paths = paths;
        _logger = logger;
    }

    public void Start()
    {
        _watcher = new FileSystemWatcher(_paths.Root)
        {
            IncludeSubdirectories = true,
            InternalBufferSize = 64 * 1024,
            NotifyFilter = NotifyFilters.FileName | NotifyFilters.DirectoryName | NotifyFilters.LastWrite | NotifyFilters.Size,
        };

        _watcher.Created += OnCreated;
        _watcher.Changed += OnChanged;
        _watcher.Deleted += OnDeleted;
        _watcher.Renamed += OnRenamed;
        _watcher.Error += OnError;
        _watcher.EnableRaisingEvents = true;
    }

    private void OnCreated(object sender, FileSystemEventArgs e) => DispatchUpsert(e.FullPath);

    private void OnChanged(object sender, FileSystemEventArgs e)
    {
        // 目录不处理 Changed（内容无意义），仅文件
        if (File.Exists(e.FullPath))
        {
            DispatchUpsert(e.FullPath);
        }
    }

    private void OnDeleted(object sender, FileSystemEventArgs e)
    {
        if (IsInternal(e.FullPath))
        {
            return;
        }

        Deleted?.Invoke(e.FullPath);
    }

    private void OnRenamed(object sender, RenamedEventArgs e)
    {
        if (IsInternal(e.FullPath) || IsInternal(e.OldFullPath))
        {
            return;
        }

        Moved?.Invoke(e.OldFullPath, e.FullPath);
    }

    private void OnError(object sender, ErrorEventArgs e)
    {
        _logger.LogWarning(e.GetException(), "文件监听缓冲溢出，触发全量扫描兜底");
        Overflowed?.Invoke();
    }

    private void DispatchUpsert(string fullPath)
    {
        if (string.Equals(fullPath, _paths.ConfigFile, StringComparison.Ordinal))
        {
            ConfigChanged?.Invoke();
            return;
        }

        if (string.Equals(fullPath, _paths.CategoriesFile, StringComparison.Ordinal) ||
            string.Equals(fullPath, _paths.TagsFile, StringComparison.Ordinal))
        {
            RegistryChanged?.Invoke();
            return;
        }

        if (IsInternal(fullPath))
        {
            return;
        }

        FileUpsert?.Invoke(fullPath);
    }

    private bool IsInternal(string fullPath)
    {
        var rel = _paths.ToRelative(fullPath);
        return rel is null || LibraryPaths.IsInternal(rel);
    }

    public void Dispose() => _watcher?.Dispose();
}

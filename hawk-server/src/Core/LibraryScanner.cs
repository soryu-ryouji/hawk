namespace Hawk.Server.Core;

/// <summary>
/// 素材目录遍历。只读目录项信息，不读文件内容。
/// 跳过 .hawk/ 内部（回收站 .hawk/trash/ 除外），库内子树应用 config.toml 的 ignore 规则。
/// </summary>
public sealed class LibraryScanner
{
    private readonly LibraryPaths _paths;
    private readonly LibraryConfig _config;

    public LibraryScanner(LibraryPaths paths, LibraryConfig config)
    {
        _paths = paths;
        _config = config;
    }

    /// <summary>遍历整个素材库（含回收站），产出全部文件绝对路径</summary>
    /// <param name="onEnumerationError">目录枚举失败（权限不足/遍历期间被删等瞬时错误）时回调；调用方据此判定遍历结果不完整</param>
    public IEnumerable<string> WalkLibrary(Action? onEnumerationError = null) =>
        WalkDirectory(_paths.Root, isTrashSubtree: false, onEnumerationError);

    /// <summary>遍历指定目录（目录创建事件后的补扫）</summary>
    public IEnumerable<string> WalkDirectory(string absDir) =>
        WalkDirectory(absDir, isTrashSubtree: IsTrashPath(absDir), onEnumerationError: null);

    private bool IsTrashPath(string absDir)
    {
        var rel = _paths.ToRelative(absDir);
        return rel is not null && LibraryPaths.IsInTrash(rel + "/");
    }

    private IEnumerable<string> WalkDirectory(string absDir, bool isTrashSubtree, Action? onEnumerationError)
    {
        var pending = new Stack<string>();
        pending.Push(absDir);

        while (pending.Count > 0)
        {
            var dir = pending.Pop();
            IEnumerable<string> entries;
            try
            {
                entries = Directory.EnumerateFileSystemEntries(dir);
            }
            catch (Exception)
            {
                // 权限不足或遍历期间被删除。瞬时错误会让本次遍历不完整，回调通知调用方
                onEnumerationError?.Invoke();
                continue;
            }

            foreach (var entry in entries)
            {
                var rel = _paths.ToRelative(entry);
                if (rel is null)
                {
                    continue;
                }

                var isDir = Directory.Exists(entry);
                if (isDir)
                {
                    // .hawk/ 只深入回收站，其余内部目录不参与索引
                    if (rel == LibraryPaths.HawkDirName)
                    {
                        pending.Push(_paths.TrashDir);
                        continue;
                    }

                    var inTrash = isTrashSubtree || LibraryPaths.IsInTrash(rel + "/");
                    if (!inTrash && _config.IsIgnored(rel))
                    {
                        continue;
                    }

                    // 目录命中 ignore 时其内容由 WalkDirectory 内的文件检查兜底
                    pending.Push(entry);
                }
                else
                {
                    // 文件也要过 ignore（*.tmp 之类的模式）；回收站子树不应用
                    if (!isTrashSubtree && _config.IsIgnored(rel))
                    {
                        continue;
                    }

                    yield return entry;
                }
            }
        }
    }
}

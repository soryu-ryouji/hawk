namespace Hawk.Server.Core;

/// <summary>库内文件系统操作辅助</summary>
public static class LibraryFs
{
    /// <summary>确保文件的父目录存在</summary>
    public static void EnsureParentDir(string absFile)
    {
        var dir = Path.GetDirectoryName(absFile);
        if (dir is not null)
        {
            Directory.CreateDirectory(dir);
        }
    }

    /// <summary>
    /// 计算回收站中的可用位置（保留原目录结构）。
    /// 同名冲突时在文件名后追加 " (n)" 后缀——恢复时按回收站中的实际名称放回。
    /// </summary>
    public static string FindFreeTrashPath(LibraryPaths paths, string libraryRelPath, bool isDirectory)
    {
        var candidate = LibraryPaths.LibraryToTrashPath(libraryRelPath);
        for (var n = 1; ; n++)
        {
            var abs = paths.ToAbsolute(candidate)!;
            if (!File.Exists(abs) && !Directory.Exists(abs))
            {
                return abs;
            }

            candidate = LibraryPaths.LibraryToTrashPath(Suffixed(libraryRelPath, n, isDirectory));
        }
    }

    private static string Suffixed(string relPath, int n, bool isDirectory)
    {
        var dir = LibraryPaths.DirOf(relPath);
        var name = LibraryPaths.NameOf(relPath);
        var ext = LibraryPaths.ExtOf(relPath);

        var newName = isDirectory || ext == "" ? $"{name} ({n})" : $"{name} ({n}).{ext}";
        return dir == "" ? newName : $"{dir}/{newName}";
    }

    /// <summary>校验文件夹/文件名（不允许路径分隔符与特殊名称）</summary>
    public static bool IsValidName(string? name) =>
        !string.IsNullOrWhiteSpace(name)
        && !name.Contains('/')
        && !name.Contains('\\')
        && name is not "." and not ".."
        && name != LibraryPaths.HawkDirName;
}

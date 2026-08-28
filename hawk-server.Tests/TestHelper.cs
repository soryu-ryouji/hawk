namespace Hawk.Server.Tests;

/// <summary>临时素材库目录：每个测试独立，Dispose 时清理</summary>
public sealed class TempDir : IDisposable
{
    public string Root { get; }

    /// <summary>库外派生缓存目录（thumbnails/colors），与 Root 同级、随 Dispose 一并清理</summary>
    public string CacheRoot { get; }

    public TempDir()
    {
        Root = Path.Combine(Path.GetTempPath(), "hawk-test-" + Guid.NewGuid().ToString("N"));
        CacheRoot = Root + "-cache";
        Directory.CreateDirectory(Root);
        Directory.CreateDirectory(CacheRoot);
    }

    public string WriteFile(string relPath, byte[] content)
    {
        var abs = Path.Combine(new[] { Root }.Concat(relPath.Split('/')).ToArray());
        Directory.CreateDirectory(Path.GetDirectoryName(abs)!);
        File.WriteAllBytes(abs, content);
        return abs;
    }

    public string WriteText(string relPath, string content) => WriteFile(relPath, System.Text.Encoding.UTF8.GetBytes(content));

    public string Mkdir(string relPath)
    {
        var abs = Path.Combine(new[] { Root }.Concat(relPath.Split('/')).ToArray());
        Directory.CreateDirectory(abs);
        return abs;
    }

    public void Dispose()
    {
        try
        {
            Directory.Delete(Root, recursive: true);
            Directory.Delete(CacheRoot, recursive: true);
        }
        catch
        {
            // 临时目录清理失败不影响测试结果
        }
    }

    /// <summary>最小 PNG（1x1），供尺寸识别/缩略图相关测试使用</summary>
    public static readonly byte[] TinyPng = Convert.FromBase64String(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==");
}

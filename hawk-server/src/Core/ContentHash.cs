using Blake3;

namespace Hawk.Server.Core;

/// <summary>BLAKE3 内容哈希（hex），item id 与元数据/缩略图命名的依据</summary>
public static class ContentHash
{
    public static string HashFile(string path, CancellationToken ct = default)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 1 << 20, FileOptions.SequentialScan);
        using var hasher = Hasher.New();
        var buffer = new byte[1 << 17];
        int read;
        while ((read = stream.Read(buffer, 0, buffer.Length)) > 0)
        {
            ct.ThrowIfCancellationRequested();
            hasher.Update(buffer.AsSpan(0, read));
        }

        return hasher.Finalize().ToString();
    }

    public static string HashBytes(ReadOnlySpan<byte> data)
    {
        using var hasher = Hasher.New();
        hasher.Update(data);
        return hasher.Finalize().ToString();
    }
}

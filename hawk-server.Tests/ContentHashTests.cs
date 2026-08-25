using Hawk.Server.Core;

namespace Hawk.Server.Tests;

public class ContentHashTests
{
    [Fact]
    public void 空内容的BLAKE3标准测试向量()
    {
        // BLAKE3 官方测试向量，用于锁定算法契约（Rust 重写时必须一致）
        Assert.Equal(
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
            ContentHash.HashBytes(ReadOnlySpan<byte>.Empty));
    }

    [Fact]
    public void 文件与字节流哈希一致()
    {
        using var dir = new TempDir();
        var content = "hawk 测试内容"u8.ToArray();
        var file = dir.WriteFile("a.bin", content);

        Assert.Equal(ContentHash.HashBytes(content), ContentHash.HashFile(file));
    }

    [Fact]
    public void 哈希为64位小写hex()
    {
        var hash = ContentHash.HashBytes("abc"u8.ToArray());
        Assert.Equal(64, hash.Length);
        Assert.All(hash, c => Assert.True(char.IsAsciiHexDigitLower(c)));
    }
}

//! BLAKE3 内容哈希（hex），item id 与元数据/缩略图命名的依据
//! 与 C# ContentHash 语义一致（流式读取，共享读打开——哈希计算期间不阻塞文件的移动/删除）

use std::io::Read;

pub fn hash_file(path: &str) -> std::io::Result<String> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, file);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 1 << 17];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn hash_bytes(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blake3_known_vector() {
        // BLAKE3 官方测试向量（"abc"）
        assert_eq!(
            hash_bytes(b"abc"),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
        assert_eq!(
            hash_bytes(b""),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }
}

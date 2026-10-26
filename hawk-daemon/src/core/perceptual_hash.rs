//! 64 位感知哈希：image_hasher 的 DCT 预处理 + Mean 算法。
//! 固定 8×8 输出与稳定 Lanczos3 resize，不启用库的不稳定 resize 特性。

use image_hasher::{HashAlg, HasherConfig};

/// 算法与存储格式的共同版本；变更参数/算法时递增，旧值由维护任务重算
pub const PHASH_VERSION: u32 = 1;

/// 计算 64 位 pHash。字节序不影响后续 XOR 汉明距离，十六进制仅用于 SQLite 展示
pub fn compute(image: &image::DynamicImage) -> u64 {
    let hash = HasherConfig::new()
        .hash_alg(HashAlg::Mean)
        .preproc_dct()
        .to_hasher()
        .hash_image(image);
    let bytes = hash.as_bytes();
    debug_assert_eq!(bytes.len(), 8);
    bytes
        .iter()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte))
}

pub fn distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterned_image() -> image::DynamicImage {
        let mut image = image::RgbaImage::new(64, 64);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let checker = if (x / 8 + y / 8) % 2 == 0 { 220 } else { 30 };
            *pixel = image::Rgba([checker, (x * 4) as u8, (y * 4) as u8, 255]);
        }
        image::DynamicImage::ImageRgba8(image)
    }

    #[test]
    fn is_deterministic_and_close_after_resize() {
        let source = patterned_image();
        let original = compute(&source);
        assert_eq!(original, compute(&source));

        let resized = image::DynamicImage::ImageRgba8(image::imageops::resize(
            &source,
            32,
            32,
            image::imageops::FilterType::Lanczos3,
        ));
        assert!(distance(original, compute(&resized)) <= 4);
    }

    #[test]
    fn recompression_stays_close_and_other_images_differ() {
        let source = patterned_image();
        let mut encoded = std::io::Cursor::new(Vec::new());
        source
            .write_to(&mut encoded, image::ImageFormat::Jpeg)
            .unwrap();
        let decoded =
            image::load_from_memory_with_format(encoded.get_ref(), image::ImageFormat::Jpeg)
                .unwrap();
        assert!(distance(compute(&source), compute(&decoded)) <= 8);

        let other = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 64, |x, _| {
            image::Rgba([(x * 4) as u8, 20, 240, 255])
        }));
        assert_ne!(compute(&source), compute(&other));
    }

    #[test]
    fn hamming_distance_counts_different_bits() {
        assert_eq!(distance(0b1010, 0b0011), 2);
        assert_eq!(distance(u64::MAX, 0), 64);
    }
}

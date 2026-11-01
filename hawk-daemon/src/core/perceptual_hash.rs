//! 64 位感知哈希：image_hasher 的 Gradient（dHash）算法。
//! 比较缩放后相邻像素的明暗关系，避免 DCT 直流分量和系数并列阈值造成的不稳定。
//! 固定 8×8 输出与稳定 Lanczos3 resize，不启用库的不稳定 resize 特性。

use image_hasher::{HashAlg, HasherConfig};

/// 算法与存储格式的共同版本；变更参数/算法时递增，旧值由维护任务重算
pub const PHASH_VERSION: u32 = 3;

pub const RGB_GRID_SIZE: u32 = 8;
pub const RGB_GRID_BYTES: usize = RGB_GRID_SIZE as usize * RGB_GRID_SIZE as usize * 3;
pub type RgbGrid = [u8; RGB_GRID_BYTES];

/// 计算 64 位感知哈希。字节序不影响后续 XOR 汉明距离，十六进制仅用于 SQLite 展示
pub fn compute(image: &image::DynamicImage) -> u64 {
    let hash = HasherConfig::new()
        .hash_alg(HashAlg::Gradient)
        .to_hasher()
        .hash_image(image);
    let bytes = hash.as_bytes();
    debug_assert_eq!(bytes.len(), 8);
    bytes
        .iter()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte))
}

/// 8×8 RGB 平均色网格：补足 dHash 缺失的颜色与空间布局信息
pub fn compute_rgb_grid(image: &image::DynamicImage) -> RgbGrid {
    let resized = image::DynamicImage::ImageRgba8(image::imageops::resize(
        image,
        RGB_GRID_SIZE,
        RGB_GRID_SIZE,
        image::imageops::FilterType::Lanczos3,
    ))
    .to_rgb8();
    let mut grid = [0; RGB_GRID_BYTES];
    grid.copy_from_slice(resized.as_raw());
    grid
}

pub fn distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// 归一化 RGB RMS 差异（0 完全相同，1 最大差异）
pub fn rgb_distance(a: &RgbGrid, b: &RgbGrid) -> f64 {
    let sum = a
        .iter()
        .zip(b)
        .map(|(&left, &right)| {
            let delta = f64::from(left) - f64::from(right);
            delta * delta
        })
        .sum::<f64>();
    (sum / (RGB_GRID_BYTES as f64 * 255.0 * 255.0)).sqrt()
}

/// RGB 复核随候选宽松度小幅放宽，但始终保持比单独 dHash 更严格的第二道门禁
pub fn rgb_threshold(max_distance: u32) -> f64 {
    0.06 + 0.003 * f64::from(max_distance)
}

pub fn similarity_matches(hash_distance: u32, rgb_difference: f64, max_distance: u32) -> bool {
    hash_distance <= max_distance && rgb_difference <= rgb_threshold(max_distance)
}

pub fn rgb_grid_to_hex(grid: &RgbGrid) -> String {
    grid.iter().map(|value| format!("{value:02x}")).collect()
}

pub fn rgb_grid_from_hex(hex: &str) -> Option<RgbGrid> {
    if hex.len() != RGB_GRID_BYTES * 2 {
        return None;
    }
    let mut grid = [0; RGB_GRID_BYTES];
    for (index, value) in grid.iter_mut().enumerate() {
        *value = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(grid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterned_image() -> image::DynamicImage {
        // 连续色调夹具：提供稳定的明暗变化，覆盖缩放与重压缩的真实图像特征
        let image = image::RgbaImage::from_fn(64, 64, |x, y| {
            let x = f64::from(x);
            let y = f64::from(y);
            let channel = |value: f64| value.clamp(0.0, 255.0) as u8;
            image::Rgba([
                channel(128.0 + 72.0 * (x / 9.0).sin() + 24.0 * (y / 13.0).cos()),
                channel(120.0 + 64.0 * (y / 11.0).sin() + 32.0 * (x / 17.0).cos()),
                channel(112.0 + 56.0 * ((x + y) / 14.0).sin()),
                255,
            ])
        });
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
        let resize_distance = distance(original, compute(&resized));
        assert!(resize_distance <= 4, "resize distance: {resize_distance}");
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
        let recompression_distance = distance(compute(&source), compute(&decoded));
        assert!(
            recompression_distance <= 5,
            "recompression distance: {recompression_distance}"
        );

        let other = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 64, |x, _| {
            image::Rgba([(x * 4) as u8, 20, 240, 255])
        }));
        // DCT+Mean 会被直流分量主导，不同画面也可能只差 1 位；Gradient 必须保持足够的区分度
        assert!(distance(compute(&source), compute(&other)) > 10);
    }

    #[test]
    fn rgb_grid_tolerates_resize_and_recompression() {
        let source = patterned_image();
        let original = compute_rgb_grid(&source);
        let resized = image::DynamicImage::ImageRgba8(image::imageops::resize(
            &source,
            32,
            32,
            image::imageops::FilterType::Lanczos3,
        ));
        assert!(rgb_distance(&original, &compute_rgb_grid(&resized)) <= rgb_threshold(5));

        let mut encoded = std::io::Cursor::new(Vec::new());
        source
            .write_to(&mut encoded, image::ImageFormat::Jpeg)
            .unwrap();
        let decoded =
            image::load_from_memory_with_format(encoded.get_ref(), image::ImageFormat::Jpeg)
                .unwrap();
        assert!(rgb_distance(&original, &compute_rgb_grid(&decoded)) <= rgb_threshold(5));
    }

    #[test]
    fn rgb_verification_rejects_greyscale_and_requires_both_stages() {
        let color = [200, 30, 40];
        let grey = [75; 3];
        let mut color_grid = [0; RGB_GRID_BYTES];
        let mut grey_grid = [0; RGB_GRID_BYTES];
        for index in 0..RGB_GRID_BYTES / 3 {
            color_grid[index * 3..index * 3 + 3].copy_from_slice(&color);
            grey_grid[index * 3..index * 3 + 3].copy_from_slice(&grey);
        }
        let difference = rgb_distance(&color_grid, &grey_grid);
        assert!(difference > rgb_threshold(20));
        assert!(!similarity_matches(0, difference, 20));
        assert!(!similarity_matches(20, 0.0, 5));
        assert!(similarity_matches(5, 0.0, 5));
    }

    #[test]
    fn rgb_grid_hex_roundtrip() {
        let grid = compute_rgb_grid(&patterned_image());
        assert_eq!(rgb_grid_from_hex(&rgb_grid_to_hex(&grid)), Some(grid));
        assert_eq!(rgb_grid_from_hex("00"), None);
    }

    #[test]
    fn hamming_distance_counts_different_bits() {
        assert_eq!(distance(0b1010, 0b0011), 2);
        assert_eq!(distance(u64::MAX, 0), 64);
    }
}

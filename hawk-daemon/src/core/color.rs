//! 调色板提炼：降采样到 64px → 中位切分量化到 ≤10 色 → 像素占比统计。
//! 提炼结果作为「内容的纯函数」写入素材元数据 TOML（参与同步，一台计算全平台复用）。
//!
//! 算法注记：Wu 量化器在 Rust 生态没有成熟实现，按 color-search.md 的预设机制
//! 采用中位切分（v2）。原理见 docs/backend/color-search.md。

use crate::core::item::PaletteColor;

/// 调色板最大颜色数
pub const PALETTE_SIZE: usize = 10;

/// 提炼前降采样到的最大边长（提速并抹掉噪点）
pub const ANALYSIS_SIZE: u32 = 64;

/// 透明像素的 alpha 阈值：低于该值不参与统计
const ALPHA_THRESHOLD: u8 = 128;

/// 从图像文件提炼调色板。源文件一般是已有的小尺寸缩略图，解码代价极低。
/// 解码失败返回 None（下次重试）；图像无有效像素返回空数组（缓存之，不再重试）
pub fn extract(image_abs: &str) -> Option<Vec<PaletteColor>> {
    let image = image::open(image_abs).ok()?.to_rgba8();
    Some(extract_from_rgba(&image))
}

/// 从已解码 RGBA 图像提炼调色板（动图取首帧）：降采样到 AnalysisSize 后中位切分，
/// 统计各代表色的像素占比（alpha < 128 的像素不参与），按占比降序取前 PaletteSize 个
pub fn extract_from_rgba(image: &image::RgbaImage) -> Vec<PaletteColor> {
    let (w, h) = image.dimensions();
    let scale = (ANALYSIS_SIZE as f64 / w as f64)
        .min(ANALYSIS_SIZE as f64 / h as f64)
        .min(1.0);
    let dst_w = ((w as f64 * scale).round() as u32).max(1);
    let dst_h = ((h as f64 * scale).round() as u32).max(1);
    let small = image::imageops::resize(image, dst_w, dst_h, image::imageops::FilterType::Triangle);

    let pixels: Vec<[u8; 3]> = small
        .pixels()
        .filter(|p| p.0[3] >= ALPHA_THRESHOLD)
        .map(|p| [p.0[0], p.0[1], p.0[2]])
        .collect();
    if pixels.is_empty() {
        return Vec::new();
    }
    median_cut(&pixels)
}

/// 中位切分：把像素按颜色空间递归切成 ≤PALETTE_SIZE 个盒子，每盒取平均色为代表色
fn median_cut(pixels: &[[u8; 3]]) -> Vec<PaletteColor> {
    let total = pixels.len();
    // 盒子 = 像素索引区间（用 Vec<usize> 保持简单，64x64=4096 像素规模极小）
    let mut boxes: Vec<Vec<usize>> = vec![(0..pixels.len()).collect()];

    while boxes.len() < PALETTE_SIZE {
        // 找 channel 跨度最大的盒子；无可分的（单色）即收敛
        let mut best: Option<usize> = None;
        let mut best_range = 0u32;
        let mut best_channel = 0usize;
        for (i, bx) in boxes.iter().enumerate() {
            if bx.len() < 2 {
                continue;
            }
            for ch in 0..3 {
                let mut min = u8::MAX;
                let mut max = u8::MIN;
                for &idx in bx {
                    let v = pixels[idx][ch];
                    min = min.min(v);
                    max = max.max(v);
                }
                let range = (max - min) as u32;
                if range > best_range {
                    best = Some(i);
                    best_range = range;
                    best_channel = ch;
                }
            }
        }
        let Some(i) = best else { break };
        let bx = boxes.remove(i);
        let mut sorted = bx;
        sorted.sort_by_key(|&idx| (pixels[idx][best_channel], pixels[idx][(best_channel + 1) % 3], pixels[idx][(best_channel + 2) % 3]));
        let mid = sorted.len() / 2;
        let (a, b) = sorted.split_at(mid);
        boxes.push(a.to_vec());
        boxes.push(b.to_vec());
    }

    let mut out: Vec<PaletteColor> = boxes
        .iter()
        .filter(|bx| !bx.is_empty())
        .map(|bx| {
            let (mut rs, mut gs, mut bs) = (0u64, 0u64, 0u64);
            for &idx in bx {
                rs += pixels[idx][0] as u64;
                gs += pixels[idx][1] as u64;
                bs += pixels[idx][2] as u64;
            }
            let n = bx.len() as u64;
            // 占比：0–100，保留 1 位小数，四舍五入
            let percentage = (bx.len() as f64 * 1000.0 / total as f64).round() / 10.0;
            PaletteColor::from_rgb(
                (rs / n) as u8,
                (gs / n) as u8,
                (bs / n) as u8,
                percentage as f32,
            )
        })
        .collect();

    // 按占比降序；并列按 RGB 升序保证确定性
    out.sort_by(|a, b| {
        b.percentage
            .partial_cmp(&a.percentage)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (a.r, a.g, a.b).cmp(&(b.r, b.g, b.b)))
    });
    out.truncate(PALETTE_SIZE);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_png(r: u8, g: u8, b: u8) -> image::RgbaImage {
        image::RgbaImage::from_pixel(8, 8, image::Rgba([r, g, b, 255]))
    }

    #[test]
    fn solid_image_yields_single_color_full_percentage() {
        let palette = extract_from_rgba(&solid_png(255, 0, 0));
        assert_eq!(palette.len(), 1);
        assert_eq!((palette[0].r, palette[0].g, palette[0].b), (255, 0, 0));
        assert!((palette[0].percentage - 100.0).abs() < 1e-6);
    }

    #[test]
    fn two_halves_split_evenly() {
        let mut img = image::RgbaImage::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                let c = if x < 4 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) };
                img.put_pixel(x, y, c);
            }
        }
        let palette = extract_from_rgba(&img);
        assert_eq!(palette.len(), 2);
        assert!((palette[0].percentage - 50.0).abs() < 1e-6);
        assert!((palette[1].percentage - 50.0).abs() < 1e-6);
    }

    #[test]
    fn fully_transparent_yields_empty() {
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 0]));
        assert!(extract_from_rgba(&img).is_empty());
    }

    #[test]
    fn percentages_sum_to_hundred() {
        // 渐变图：占比合计应≈100
        let mut img = image::RgbaImage::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                img.put_pixel(x, y, image::Rgba([(x * 16) as u8, (y * 16) as u8, 128, 255]));
            }
        }
        let palette = extract_from_rgba(&img);
        let sum: f32 = palette.iter().map(|p| p.percentage).sum();
        assert!((sum - 100.0).abs() < 1.0, "sum = {sum}");
        assert!(palette.len() <= PALETTE_SIZE);
    }
}

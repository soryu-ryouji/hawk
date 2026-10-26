//! 缩略图服务：解码（image crate）+ 缩放（fast_image_resize）+ 有损 WebP 编码
//! （webp/libwebp，quality 80）。
//! 存储于库外缓存目录（<系统缓存>/hawk/cache/<库标识>/thumbnails/<尺寸>/<hash>.webp），
//! 本地缓存可重建。固定 512/1024 两档（等比缩入边长内，不放大）：512 供网格，
//! 1024 供检查器与广域网预览；旧库已生成的 1024 缓存直接复用。
//!
//! 生成策略：
//! - 首扫/全量重扫的导入通道（pipeline 扫描并行阶段）在哈希后单次解码，
//!   同出 调色板 + 缩略图 + 宽高，item 入库即完整可显示
//! - 增量入库（监听事件/API）不生成缩略图，仅按需补调色板；缩略图由读取端惰性派发
//! - 读取端 /item/thumbnail 未命中时返回 404 并派发后台生成（thumbnail_worker），
//!   完成后经 item.updated 重试；不回源原图，避免低带宽下按卡片下载整份源文件
//! - 浏览器不可直接渲染的格式（tiff 等）同样经生成的 WebP 显示
//!
//! 共享读打开、不放大小图。

use fast_image_resize as fr;
use std::sync::Arc;

#[derive(Clone)]
pub struct ThumbnailService {
    paths: Arc<crate::core::paths::LibraryPaths>,
}

/// 网格缩略图边长（等比缩入上限，不放大）
pub const GRID_THUMBNAIL_SIZE: u32 = 512;
/// 检查器与广域网预览边长，也是未指定 size 时的默认档
pub const DEFAULT_THUMBNAIL_SIZE: u32 = 1024;
/// 有界尺寸集合：一次解码生成全部缺失档，避免任意宽度导致缓存键膨胀
pub const THUMBNAIL_SIZES: [u32; 2] = [GRID_THUMBNAIL_SIZE, DEFAULT_THUMBNAIL_SIZE];

pub fn is_supported_size(size: u32) -> bool {
    THUMBNAIL_SIZES.contains(&size)
}

impl ThumbnailService {
    pub fn new(paths: Arc<crate::core::paths::LibraryPaths>) -> ThumbnailService {
        ThumbnailService { paths }
    }

    /// 默认档路径：调色板提炼优先复用这份较大的已解码缓存
    pub fn get_path(&self, hash: &str) -> String {
        self.get_path_with_size(hash, DEFAULT_THUMBNAIL_SIZE)
    }

    pub fn get_path_with_size(&self, hash: &str, size: u32) -> String {
        format!("{}/{}/{}.webp", self.paths.thumbnails_dir, size, hash)
    }

    /// 所有派生档是否齐备；缺任一档时需要一次解码补齐
    pub fn has_all(&self, hash: &str) -> bool {
        THUMBNAIL_SIZES
            .iter()
            .all(|size| std::path::Path::new(&self.get_path_with_size(hash, *size)).is_file())
    }

    /// 读取图像尺寸(只解头部)。非图像或解码失败返回 None。
    /// with_guessed_format：按内容魔数喷探格式——扩展名与内容不符的文件
    /// （如 .png 实为 JPEG）也能识别，否则这类素材永久滞留 0×0
    pub fn identify(abs_path: &str) -> Option<(i32, i32)> {
        image::ImageReader::open(abs_path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .into_dimensions()
            .ok()
            .map(|(w, h)| (w as i32, h as i32))
    }

    /// 检测字节流的图像格式扩展名。无法识别返回 None
    pub fn detect_extension_bytes(data: &[u8]) -> Option<String> {
        let format = image::guess_format(data).ok()?;
        format_to_ext(format)
    }

    /// 内容喷探：只读文件头部字节判断是否图像格式（魔数比对，与扩展名无关）。
    /// 非图像文件（视频/文本/配置等）在解码派生前被拦下，避免无谓的整文件读取与
    /// 周期对账重试。打不开（占用/移动窗口等瞬时状态）时保守视为图像，
    /// 交给后续解码路径判定——避免给真图像误写空调色板负缓存
    pub fn is_probably_image(abs_path: &str) -> bool {
        use std::io::Read;
        let Ok(mut file) = std::fs::File::open(abs_path) else {
            return true;
        };
        let mut head = [0u8; 64];
        let n = file.read(&mut head).unwrap_or(0);
        image::guess_format(&head[..n]).is_ok()
    }

    /// 解码原图为 RGBA8 动态图。供惰性生成与扫描导入通道共用——导入通道一次解码后
    /// 共享给 调色板/缩略图/宽高。解码失败返回 None（调用方走对账重试/惰性兜底）。
    /// with_guessed_format：按内容喷探格式，扩展名错位文件同样可解码
    pub fn decode(source_abs: &str) -> Option<image::DynamicImage> {
        let image = image::ImageReader::open(source_abs)
            .ok()?
            .with_guessed_format()
            .ok()?
            .decode()
            .ok()?;
        Some(image::DynamicImage::ImageRgba8(image.to_rgba8()))
    }

    /// 为指定内容生成全部缺失缩略图；force 时重建全部档。
    /// 源文件不是图像或已消失时静默跳过——缩略图是尽力而为的缓存。返回是否实际生成了文件。
    /// 仅缺小图且默认档已存在时从默认档解码，避免再次读取和解码大尺寸原图。
    pub fn generate(&self, hash: &str, source_abs: &str, force: bool) -> bool {
        if !std::path::Path::new(source_abs).is_file() {
            return false;
        }
        if !force && self.has_all(hash) {
            return false;
        }
        // 非图像文件（内容喷探）：解码必然失败，不读文件不告警——调色板/宽高的
        // 空负缓存由扫描与 worker 的同源喷探写入，此处只管缩略图缓存
        if !Self::is_probably_image(source_abs) {
            tracing::debug!("跳过非图像文件的缩略图生成 {source_abs}");
            return false;
        }

        let default_file = self.get_path(hash);
        let decode_source = if !force && std::path::Path::new(&default_file).is_file() {
            default_file
        } else {
            source_abs.to_string()
        };
        let image = match Self::decode(&decode_source) {
            Some(i) => i,
            None => {
                // 解码失败需要可见：此类素材的缩略图/调色板会永久缺失，靠周期对账自愈重试（手动换图后自动恢复）
                tracing::warn!("缩略图解码失败 {decode_source}");
                return false;
            }
        };
        self.generate_from_image_inner(hash, &image, force)
    }

    /// 从已解码图像生成全部缺失缩略图（扫描导入通道单解码共享同一份位图）。
    pub fn generate_from_image(&self, hash: &str, image: &image::DynamicImage) -> bool {
        self.generate_from_image_inner(hash, image, false)
    }

    fn generate_from_image_inner(
        &self,
        hash: &str,
        image: &image::DynamicImage,
        force: bool,
    ) -> bool {
        let (src_w, src_h) = (image.width(), image.height());
        let mut generated = false;
        for size in THUMBNAIL_SIZES {
            let target = self.get_path_with_size(hash, size);
            if !force && std::path::Path::new(&target).is_file() {
                continue;
            }
            let mut resizer = fr::Resizer::new();
            // Max：等比缩放到边长内，不放大小图
            let scale = (size as f64 / src_w as f64)
                .min(size as f64 / src_h as f64)
                .min(1.0);
            let dst_w = ((src_w as f64 * scale).round() as u32).max(1);
            let dst_h = ((src_h as f64 * scale).round() as u32).max(1);
            let mut dst = fr::images::Image::new(dst_w, dst_h, fr::PixelType::U8x4);
            if let Err(e) = resizer.resize(
                image,
                &mut dst,
                &fr::ResizeOptions::new()
                    .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3)),
            ) {
                tracing::debug!("缩略图缩放失败 {hash}: {e}");
                continue;
            }
            let rgba = image::RgbaImage::from_raw(dst_w, dst_h, dst.buffer().to_vec())
                .expect("缩略图缓冲尺寸一致");
            let encoder = webp::Encoder::from_rgba(&rgba, dst_w, dst_h);
            let encoded = encoder.encode(80.0);
            if let Some(parent) = std::path::Path::new(&target).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&target, &*encoded) {
                // 写失败可见（占用/杀毒拦截等瞬时错误）：读取端未命中会重试生成，但对账不会主动补缩略图
                tracing::warn!("缩略图写入失败 {target}: {e}");
                continue;
            }
            generated = true;
        }
        generated
    }

    /// 删除某内容的全部缩略图
    pub fn delete(&self, hash: &str) {
        for size in THUMBNAIL_SIZES {
            let file = self.get_path_with_size(hash, size);
            if std::path::Path::new(&file).is_file() {
                let _ = std::fs::remove_file(&file);
            }
        }
    }
}

/// ImageSharp FileExtensions.First() 对齐的扩展名映射
fn format_to_ext(format: image::ImageFormat) -> Option<String> {
    Some(
        match format {
            image::ImageFormat::Jpeg => "jpg",
            image::ImageFormat::Png => "png",
            image::ImageFormat::Gif => "gif",
            image::ImageFormat::WebP => "webp",
            image::ImageFormat::Tiff => "tiff",
            image::ImageFormat::Bmp => "bmp",
            _ => return None,
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_bounded_sizes_and_repairs_missing_grid_from_default() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("library");
        let cache = temp.path().join("cache");
        std::fs::create_dir_all(&root).unwrap();
        let paths = crate::core::paths::LibraryPaths::new(
            &root.to_string_lossy(),
            Some(cache.to_string_lossy().to_string()),
        );
        paths.ensure_layout();
        let source = root.join("source.png");
        image::RgbImage::from_pixel(1600, 900, image::Rgb([20, 40, 60]))
            .save(&source)
            .unwrap();
        let service = ThumbnailService::new(Arc::new(paths));
        let source = source.to_string_lossy().to_string();

        assert!(service.generate("hash", &source, false));
        assert!(service.has_all("hash"));
        assert!(!service.generate("hash", &source, false));
        for (size, expected) in [(512, (512, 288)), (1024, (1024, 576))] {
            let file = service.get_path_with_size("hash", size);
            assert_eq!(ThumbnailService::identify(&file), Some(expected));
        }

        // 旧库只有 1024 档时，仅补 512；默认档字节保持不变，且无需重读原图
        let grid = service.get_path_with_size("hash", GRID_THUMBNAIL_SIZE);
        let default_file = service.get_path("hash");
        let default_before = std::fs::read(&default_file).unwrap();
        std::fs::remove_file(&grid).unwrap();
        // 原图换成 1×1：若错误回读原图，不放大规则会产生 1×1，而不是预期的 512×288
        image::RgbImage::from_pixel(1, 1, image::Rgb([0, 0, 0]))
            .save(&source)
            .unwrap();
        assert!(service.generate("hash", &source, false));
        assert_eq!(ThumbnailService::identify(&grid), Some((512, 288)));
        assert_eq!(std::fs::read(default_file).unwrap(), default_before);

        service.delete("hash");
        assert!(!service.has_all("hash"));
        for size in THUMBNAIL_SIZES {
            assert!(!std::path::Path::new(&service.get_path_with_size("hash", size)).exists());
        }
    }

    #[test]
    fn only_fixed_sizes_are_accepted() {
        assert!(is_supported_size(512));
        assert!(is_supported_size(1024));
        assert!(!is_supported_size(256));
        assert!(!is_supported_size(0));
    }
}

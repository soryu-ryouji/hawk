//! 缩略图服务：解码（image crate）+ 缩放（fast_image_resize）+ 有损 WebP 编码
//! （webp/libwebp，quality 80，与 C# ImageSharp WebpEncoder 对齐）。
//! 存储于库外缓存目录（<系统缓存>/hawk/cache/<库标识>/thumbnails/<size>/<hash>.webp），本地缓存可重建。
//!
//! 生成策略（与 C# 全量生成不同，Rust 版为惰性）：
//! - 入库/启动对账不生成缩略图，仅提炼调色板（pipeline 派发仅调色板任务）
//! - 读取端 /item/thumbnail 未命中时回源原图并由读取端派发后台生成（thumbnail_worker）
//! - 不可渲染格式（tiff 等）必须生成缩略图转换，否则 <img> 无法显示
//!
//! 共享读打开、不放大小图、按尺寸跳过等语义与 C# ThumbnailService 一致。

use fast_image_resize as fr;
use std::sync::Arc;

#[derive(Clone)]
pub struct ThumbnailService {
    paths: Arc<crate::core::paths::LibraryPaths>,
}

/// 浏览器 <img> 可直接渲染的扩展名。之外的可解码格式（tiff 等）必须走缩略图转换
const DIRECT_ORIGINAL_EXTS: [&str; 6] = ["jpg", "jpeg", "png", "gif", "webp", "bmp"];

impl ThumbnailService {
    pub fn new(paths: Arc<crate::core::paths::LibraryPaths>) -> ThumbnailService {
        ThumbnailService { paths }
    }

    pub fn get_path(&self, hash: &str, size: i32) -> String {
        format!("{}/{}/{}.webp", self.paths.thumbnails_dir, size, hash)
    }

    pub fn exists(&self, hash: &str, size: i32) -> bool {
        std::path::Path::new(&self.get_path(hash, size)).is_file()
    }

    /// 读取图像尺寸(只解码头信息)。非图像或解码失败返回 None
    pub fn identify(abs_path: &str) -> Option<(i32, i32)> {
        image::image_dimensions(abs_path).ok().map(|(w, h)| (w as i32, h as i32))
    }

    /// 检测字节流的图像格式扩展名。无法识别返回 None
    pub fn detect_extension_bytes(data: &[u8]) -> Option<String> {
        let format = image::guess_format(data).ok()?;
        format_to_ext(format)
    }

    /// 原图能否被浏览器直接渲染（决定读取端未命中时能否回源原图）
    pub fn is_browser_renderable(abs_path: &str) -> bool {
        DIRECT_ORIGINAL_EXTS.contains(&crate::core::paths::LibraryPaths::ext_of(abs_path).as_str())
    }

    /// 为指定内容生成全部配置尺寸的缩略图；已存在的跳过（force 时强制重建）。
    /// 源文件不是图像或已消失时静默跳过——缩略图是尽力而为的缓存。返回是否实际生成了文件
    pub fn generate(&self, hash: &str, source_abs: &str, sizes: &[i32], force: bool) -> bool {
        if !std::path::Path::new(source_abs).is_file() {
            return false;
        }
        let pending: Vec<i32> = sizes
            .iter()
            .copied()
            .filter(|s| force || !self.exists(hash, *s))
            .collect();
        if pending.is_empty() {
            return false;
        }

        let image = match image::open(source_abs) {
            Ok(i) => image::DynamicImage::ImageRgba8(i.to_rgba8()),
            Err(e) => {
                // 解码失败需要可见：此类素材的缩略图/调色板会永久缺失，靠周期对账自愈重试（手动换图后自动恢复）
                tracing::warn!("缩略图解码失败 {source_abs}: {e}");
                return false;
            }
        };
        let (src_w, src_h) = (image.width(), image.height());
        let mut resizer = fr::Resizer::new();

        let mut generated = false;
        for size in pending {
            // Max：等比缩放到边长内，不放大小图
            let scale = (size as f64 / src_w as f64)
                .min(size as f64 / src_h as f64)
                .min(1.0);
            let dst_w = ((src_w as f64 * scale).round() as u32).max(1);
            let dst_h = ((src_h as f64 * scale).round() as u32).max(1);
            let mut dst = fr::images::Image::new(dst_w, dst_h, fr::PixelType::U8x4);
            if let Err(e) = resizer.resize(
                &image,
                &mut dst,
                &fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3)),
            ) {
                tracing::debug!("缩略图缩放失败 {source_abs}@{size}: {e}");
                continue;
            }
            let rgba = image::RgbaImage::from_raw(dst_w, dst_h, dst.buffer().to_vec())
                .expect("缩略图缓冲尺寸一致");
            let encoder = webp::Encoder::from_rgba(&rgba, dst_w, dst_h);
            let encoded = encoder.encode(80.0);
            let target = self.get_path(hash, size);
            if let Some(parent) = std::path::Path::new(&target).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&target, &*encoded).is_ok() {
                generated = true;
            }
        }
        generated
    }

    /// 删除某内容的全部缩略图
    pub fn delete(&self, hash: &str, sizes: &[i32]) {
        for size in sizes {
            let file = self.get_path(hash, *size);
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

import ImageIO
import UIKit
import UniformTypeIdentifiers

nonisolated struct ShareImageError: LocalizedError, Sendable {
    let message: String
    var errorDescription: String? { message }
}

/// 分享进来的图片：落到扩展临时目录，HEIC/HEIF 转 PNG（daemon 的图片解码器不认 HEIC）。
/// HEIC 源本身有损，转 PNG 是无损的第二次编码（不叠加压缩损失）；JPEG/PNG/GIF/WebP 原样上传，不重编码。
nonisolated enum ShareImageLoader {
    struct Loaded: Identifiable, @unchecked Sendable {
        let id = UUID()
        /// 待上传文件（可能是转码后的 JPEG）
        let fileURL: URL
        let filename: String
        /// 列表缩略图（已按 EXIF 方向摆正）
        let thumbnail: UIImage

        /// 清理该图片独占的临时目录
        func cleanup() {
            try? FileManager.default.removeItem(at: fileURL.deletingLastPathComponent())
        }
    }

    /// HEIC 解码上限：覆盖 12MP 原图（4032 长边，不降采样）；
    /// 24MP/48MP 会被降到 4096——PNG 编码在扩展进程里要持有整张位图，再大容易触发内存上限
    private static let maxPixelSize = 4096
    private static let thumbnailPixelSize = 480

    static func load(from provider: NSItemProvider) async throws -> Loaded? {
        guard provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) else {
            return nil
        }
        let source = try await copyRepresentation(from: provider)
        let stem = source.deletingPathExtension().lastPathComponent
        let ext = source.pathExtension.lowercased()
        let isHEIC = ext == "heic" || ext == "heif"

        var uploadURL = source
        var filename = source.lastPathComponent
        if isHEIC {
            let png = source.deletingLastPathComponent().appendingPathComponent("\(stem).png")
            guard let image = downscaledImage(at: source) else {
                throw ShareImageError(message: "图片无法解码：\(filename)")
            }
            guard writePNG(image, to: png) else {
                throw ShareImageError(message: "图片转码失败：\(filename)")
            }
            uploadURL = png
            filename = "\(stem).png"
        }

        guard let thumbnail = thumbnail(at: source) else {
            throw ShareImageError(message: "图片无法读取：\(filename)")
        }
        return Loaded(fileURL: uploadURL, filename: filename, thumbnail: thumbnail)
    }

    /// `loadFileRepresentation` 给的临时文件只在回调内有效，必须立刻复制到自己的目录
    private static func copyRepresentation(from provider: NSItemProvider) async throws -> URL {
        try await withCheckedThrowingContinuation { continuation in
            provider.loadFileRepresentation(forTypeIdentifier: UTType.image.identifier) {
                url, error in
                if let error {
                    continuation.resume(throwing: error)
                    return
                }
                guard let url else {
                    continuation.resume(
                        throwing: ShareImageError(message: "图片内容为空"))
                    return
                }
                let dir = FileManager.default.temporaryDirectory
                    .appendingPathComponent("hawk-share-\(UUID().uuidString)", isDirectory: true)
                do {
                    try FileManager.default.createDirectory(
                        at: dir, withIntermediateDirectories: true)
                    let destination = dir.appendingPathComponent(url.lastPathComponent)
                    try FileManager.default.copyItem(at: url, to: destination)
                    continuation.resume(returning: destination)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    private static func downscaledImage(at url: URL) -> CGImage? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
            kCGImageSourceShouldCacheImmediately: true,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
    }

    /// PNG 无质量参数：解码出的位图原样写盘（HEIC 的解码误差已存在，这里不叠加第二次有损编码）
    private static func writePNG(_ image: CGImage, to url: URL) -> Bool {
        guard
            let destination = CGImageDestinationCreateWithURL(
                url as CFURL, UTType.png.identifier as CFString, 1, nil)
        else { return false }
        CGImageDestinationAddImage(destination, image, nil)
        return CGImageDestinationFinalize(destination)
    }

    private static func thumbnail(at url: URL) -> UIImage? {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: thumbnailPixelSize,
        ]
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
        else { return nil }
        return UIImage(cgImage: image)
    }
}

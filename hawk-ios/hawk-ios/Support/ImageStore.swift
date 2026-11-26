import Foundation
import UIKit

/// 缩略图与原图的下载/解码缓存。内容寻址的缩略图可长期缓存，原图按降采样尺寸缓存。
@MainActor
final class ImageStore {
    static let shared = ImageStore()

    private let thumbnails = NSCache<NSString, UIImage>()
    private let originals = NSCache<NSString, UIImage>()
    private var inflight: [String: Task<UIImage?, Never>] = [:]
    private let session: URLSession

    private init() {
        let configuration = URLSessionConfiguration.default
        configuration.urlCache = URLCache(
            memoryCapacity: 64 * 1024 * 1024, diskCapacity: 512 * 1024 * 1024)
        configuration.requestCachePolicy = .returnCacheDataElseLoad
        configuration.timeoutIntervalForRequest = 60
        session = URLSession(configuration: configuration)
        thumbnails.countLimit = 400
        thumbnails.totalCostLimit = 180 * 1024 * 1024
        originals.countLimit = 24
        originals.totalCostLimit = 320 * 1024 * 1024
    }

    func thumbnail(for url: URL, sessionToken: String = "", mediaRevision: Int = 0) async
        -> UIImage?
    {
        await image(
            for: url, maxPixelSize: 1024, cache: thumbnails, sessionToken: sessionToken,
            mediaRevision: mediaRevision)
    }

    func original(
        for url: URL, maxPixelSize: Int = 4096, sessionToken: String = "", mediaRevision: Int = 0
    ) async -> UIImage? {
        await image(
            for: url, maxPixelSize: maxPixelSize, cache: originals, sessionToken: sessionToken,
            mediaRevision: mediaRevision)
    }

    private func image(
        for url: URL, maxPixelSize: Int, cache: NSCache<NSString, UIImage>, sessionToken: String,
        mediaRevision: Int
    ) async -> UIImage? {
        let cacheKey = "\(url.absoluteString)#\(maxPixelSize)#\(mediaRevision)" as NSString
        if let cached = cache.object(forKey: cacheKey) {
            return cached
        }
        let key = cacheKey as String
        if let existing = inflight[key] {
            return await existing.value
        }

        let downloads = session
        let task = Task<UIImage?, Never> {
            do {
                var request = URLRequest(url: url)
                if mediaRevision > 0 {
                    // 强制重建后的同 URL 内容必须绕过此前成功的磁盘缓存。
                    request.cachePolicy = .reloadIgnoringLocalCacheData
                }
                APIClient.authorize(&request, credential: sessionToken)
                let (data, response) = try await downloads.data(for: request)
                guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode)
                else {
                    return nil
                }
                return Self.downsample(data, maxPixelSize: maxPixelSize) ?? UIImage(data: data)
            } catch {
                return nil
            }
        }
        inflight[key] = task
        let image = await task.value
        inflight.removeValue(forKey: key)
        if let image {
            cache.setObject(image, forKey: cacheKey, cost: Self.cost(of: image))
        }
        return image
    }

    /// 带重试的加载：新入库素材的缩略图/原图可能短暂 404，失败后稍等再试，
    /// 避免网格与预览留下永久占位
    func loadWithRetry(
        for url: URL, maxPixelSize: Int, sessionToken: String = "", mediaRevision: Int = 0
    ) async -> UIImage? {
        for attempt in 0..<3 {
            let image =
                if maxPixelSize <= 1024 {
                    await thumbnail(
                        for: url, sessionToken: sessionToken, mediaRevision: mediaRevision)
                } else {
                    await original(
                        for: url, maxPixelSize: maxPixelSize, sessionToken: sessionToken,
                        mediaRevision: mediaRevision)
                }
            if Task.isCancelled { return nil }
            if let image { return image }
            if attempt < 2 {
                try? await Task.sleep(nanoseconds: 1_500_000_000)
            }
        }
        return nil
    }

    /// 预取相邻页的原图（左右滑动前先落缓存），失败静默
    func prefetchOriginals(_ urls: [URL], maxPixelSize: Int = 4096, sessionToken: String = "") {
        for url in urls {
            Task(priority: .utility) {
                _ = await original(for: url, maxPixelSize: maxPixelSize, sessionToken: sessionToken)
            }
        }
    }

    func storeThumbnail(_ image: UIImage, for url: URL) {
        thumbnails.setObject(
            image, forKey: "\(url.absoluteString)#1024#0" as NSString, cost: Self.cost(of: image))
    }

    private nonisolated static func cost(of image: UIImage) -> Int {
        guard let cgImage = image.cgImage else { return 0 }
        return cgImage.bytesPerRow * cgImage.height
    }

    nonisolated private static func downsample(_ data: Data, maxPixelSize: Int) -> UIImage? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else {
            return nil
        }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: false,
            kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
        ]
        guard let cgImage = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
        else {
            return nil
        }
        return UIImage(cgImage: cgImage)
    }
}

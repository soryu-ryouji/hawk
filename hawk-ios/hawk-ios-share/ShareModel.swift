import Foundation
import Observation
import UIKit

nonisolated struct ShareSetupError: LocalizedError, Sendable {
    let message: String
    var errorDescription: String? { message }
}

/// 分享扩展开关：目标探测 → 读分类/标签 → 逐张上传。
/// 只走局域网直连（远端库保存是后续里程碑：需要写会话与远端写白名单配合）。
@MainActor
@Observable
final class ShareModel {
    enum Phase: Equatable {
        case preparing
        case ready
        case uploading(done: Int, total: Int)
        case finished(saved: Int, skipped: Int)
        case failed(String)

        var isBusy: Bool {
            switch self {
            case .preparing, .uploading: true
            case .ready, .finished, .failed: false
            }
        }
    }

    private(set) var phase: Phase = .preparing
    private(set) var images: [ShareImageLoader.Loaded] = []
    private(set) var categories: [String] = []
    private(set) var tags: [String] = []
    private(set) var targetLabel = ""
    var selectedCategories: Set<String> = []
    var selectedTags: Set<String> = []

    private var client: ShareAPIClient?

    /// 单次分享最多处理张数（扩展内存与前台时间都有限）
    private static let maxImages = 20

    func prepare(providers: [NSItemProvider]) async {
        do {
            guard let target = ShareTargetStore.load() else {
                throw ShareSetupError(
                    message: "还没有可用的局域网素材库：请先在 hawk App 里连接素材库（分享扩展目前只支持局域网保存）")
            }
            targetLabel = target.label
            let client = try await connect(target)
            self.client = client

            // 分类/标签拉取失败不阻断保存：只是少选两项
            async let categoriesTask = client.categories()
            async let tagsTask = client.tags()
            categories = ((try? await categoriesTask) ?? []).sorted()
            tags = ((try? await tagsTask) ?? []).sorted()

            // 逐张读取，控制内存峰值（分享 20 张大图时不要一次性全解码）
            var loaded: [ShareImageLoader.Loaded] = []
            for provider in providers.prefix(Self.maxImages) {
                if let image = try? await ShareImageLoader.load(from: provider) {
                    loaded.append(image)
                }
            }
            guard !loaded.isEmpty else {
                throw ShareSetupError(message: "没有可保存的图片")
            }
            images = loaded
            phase = .ready
        } catch {
            phase = .failed(Self.message(for: error))
        }
    }

    /// 串行上传；任一张失败即停并给出原因（已成功的仍保留在库内）
    func upload() async -> Bool {
        guard let client, !images.isEmpty else { return false }
        let total = images.count
        var saved = 0
        var skipped = 0
        phase = .uploading(done: 0, total: total)
        for (index, image) in images.enumerated() {
            do {
                let duplicate = try await client.upload(
                    fileURL: image.fileURL,
                    filename: image.filename,
                    categories: Array(selectedCategories),
                    tags: Array(selectedTags))
                if duplicate {
                    skipped += 1
                } else {
                    saved += 1
                }
                phase = .uploading(done: index + 1, total: total)
            } catch let error as ShareAPIError where error.isDuplicate {
                // 同名文件已存在且内容不同：按「已收藏过」处理，不打断整批
                skipped += 1
                phase = .uploading(done: index + 1, total: total)
            } catch {
                phase = .failed(Self.message(for: error))
                return false
            }
        }
        phase = .finished(saved: saved, skipped: skipped)
        return true
    }

    func cleanup() {
        for image in images {
            image.cleanup()
        }
        images = []
    }

    private func connect(_ target: ShareTargetStore.Target) async throws -> ShareAPIClient {
        var lastError: Error = ShareSetupError(message: "素材库地址不可用")
        for raw in target.urls {
            let candidate =
                raw.hasPrefix("http://") || raw.hasPrefix("https://")
                ? raw : "http://\(raw)"
            guard let base = URL(string: candidate) else { continue }
            let client = ShareAPIClient(baseURL: base, token: target.token)
            do {
                _ = try await client.libraryName()
                return client
            } catch {
                lastError = error
            }
        }
        throw lastError
    }

    private static func message(for error: Error) -> String {
        if let api = error as? ShareAPIError {
            if api.isUnauthorized {
                return "连接凭证已失效：回到 hawk App 重新连接素材库后再试"
            }
            if api.isReadOnly {
                return "当前连接没有写权限：在桌面端重新生成配对二维码时勾选「写权限」，手机重新扫码"
            }
            return api.message
        }
        if let setup = error as? ShareSetupError {
            return setup.message
        }
        if let image = error as? ShareImageError {
            return image.message
        }
        if let urlError = error as? URLError {
            return "无法连接素材库（\(urlError.localizedDescription)）：确认手机与电脑在同一局域网、桌面端 hawk 正在运行"
        }
        return error.localizedDescription
    }
}

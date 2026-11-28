import Foundation
import Observation
import UIKit

nonisolated struct ShareSetupError: LocalizedError, Sendable {
    let message: String
    var errorDescription: String? { message }
}

/// 分享扩展开关：目标连接 → 读分类/标签 → 逐张上传。
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

    private static let maxImages = 20

    func prepare(providers: [NSItemProvider]) async {
        do {
            guard let target = ShareTargetStore.load() else {
                throw ShareSetupError(message: "还没有可用的素材库：请先在 hawk App 里连接素材库")
            }
            targetLabel = target.label
            let connected = try await Self.connect(target)
            client = connected

            async let categoriesTask = connected.categories()
            async let tagsTask = connected.tags()
            categories = ((try? await categoriesTask) ?? []).sorted()
            tags = ((try? await tagsTask) ?? []).sorted()

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

    /// 串行上传；任一张失败即停并给出原因（已成功的仍保留在库内）。
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
        client = nil
    }

    nonisolated private static func connect(_ target: ShareTargetStore.Target) async throws
        -> ShareAPIClient
    {
        guard !target.accessKey.isEmpty else {
            throw ShareSetupError(message: "连接缺少 AccessKey，请先在 hawk App 里重新连接素材库")
        }
        var lastError: Error = ShareSetupError(message: "素材库地址不可用")
        for raw in target.urls {
            guard let base = normalizedURL(raw) else { continue }
            let client = ShareAPIClient(baseURL: base, token: target.accessKey)
            do {
                _ = try await client.libraryName()
                return client
            } catch {
                lastError = error
            }
        }
        throw lastError
    }

    nonisolated private static func normalizedURL(_ raw: String) -> URL? {
        var value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return nil }
        if !value.contains("://") {
            value = "http://\(value)"
        }
        guard let url = URL(string: value), url.scheme?.lowercased() == "http" else {
            return nil
        }
        return url
    }

    private static func message(for error: Error) -> String {
        if let api = error as? ShareAPIError {
            if api.isUnauthorized {
                return "连接凭证已失效：回到 hawk App 重新连接素材库后再试"
            }
            if api.isReadOnly {
                return "当前连接没有导入权限：请使用具备导入或读写权限的 AccessKey 重新配对"
            }
            if api.isNameConflict {
                return "素材库根目录已有同名但内容不同的文件，请先重命名图片后再保存"
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
            return "无法连接素材库（\(urlError.localizedDescription)）：确认目标设备在线且与本机处于同一局域网"
        }
        return error.localizedDescription
    }
}

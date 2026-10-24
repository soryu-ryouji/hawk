import Foundation
import Observation
import SwiftUI
import UIKit

struct ShareFile: Identifiable {
    let id = UUID()
    let url: URL
}

/// 原图导出：保存到相册 / 分享。两条路径都先取原图数据（预览里看到的可能是缩略图），
/// 分享用系统分享面板，文件落在临时目录，会话结束由系统回收。
@MainActor
@Observable
final class MediaExporter {
    private(set) var busyItemID: String?
    var toast: String?
    var error: String?
    var shareFile: ShareFile?

    var isBusy: Bool { busyItemID != nil }

    func isBusy(_ itemID: String) -> Bool { busyItemID == itemID }

    func save(_ item: SkeletonItem, api: APIClient) async {
        guard busyItemID == nil else { return }
        busyItemID = item.id
        defer { busyItemID = nil }
        do {
            let data = try await api.fetchFileData(id: item.id)
            try await PhotoSaver.save(data)
            Haptics.success()
            toast = "已保存到相册"
        } catch {
            Haptics.warning()
            self.error = message(for: error)
        }
    }

    func share(_ item: SkeletonItem, name: String, api: APIClient) async {
        guard busyItemID == nil else { return }
        busyItemID = item.id
        defer { busyItemID = nil }
        do {
            let data = try await api.fetchFileData(id: item.id)
            let directory = FileManager.default.temporaryDirectory
                .appendingPathComponent("hawk-share", isDirectory: true)
            try FileManager.default.createDirectory(
                at: directory, withIntermediateDirectories: true)
            let url = directory.appendingPathComponent(name)
            try data.write(to: url, options: .atomic)
            shareFile = ShareFile(url: url)
        } catch {
            Haptics.warning()
            self.error = message(for: error)
        }
    }

    func clearError() { error = nil }

    private func message(for error: any Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }
}

/// 系统分享面板（UIActivityViewController）
struct ShareSheet: UIViewControllerRepresentable {
    let url: URL

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [url], applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}

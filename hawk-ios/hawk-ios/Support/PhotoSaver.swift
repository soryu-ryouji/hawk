import Foundation
import Photos

nonisolated enum PhotoSaveError: LocalizedError, Sendable {
    case denied
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .denied: "没有相册写入权限，请在系统设置中允许 hawk 添加照片"
        case .failed(let message): message
        }
    }
}

nonisolated enum PhotoSaver {
    static func save(_ data: Data) async throws {
        let current = PHPhotoLibrary.authorizationStatus(for: .addOnly)
        let status: PHAuthorizationStatus
        if current == .notDetermined {
            status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        } else {
            status = current
        }
        guard status == .authorized || status == .limited else {
            throw PhotoSaveError.denied
        }
        do {
            try await PHPhotoLibrary.shared().performChanges {
                let request = PHAssetCreationRequest.forAsset()
                request.addResource(with: .photo, data: data, options: nil)
            }
        } catch {
            throw PhotoSaveError.failed(error.localizedDescription)
        }
    }
}

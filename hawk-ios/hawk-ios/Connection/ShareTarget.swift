import Foundation

/// 共享给分享扩展的目标（App Group UserDefaults）。
/// 原生 LAN/WAN 都保存 rendezvous 地址与目标身份，由扩展独立建立加密会话；
/// 没有 targetIdentity 的手工 LAN 才直接访问 LAN Web。
nonisolated enum ShareTarget {
    static let appGroupID = "group.com.soryu-ryouji.hawk"
    static let storageKey = "shareTarget.v2"

    struct Value: Codable, Sendable {
        var urls: [String]
        var accessKey: String
        var targetIdentity: String
        var targetDeviceId: String
        /// 展示名：连接名 · 素材库名
        var label: String
    }

    static func save(profile: ConnectionProfile, libraryName: String) {
        let urls = (try? profile.candidateBaseURLs().map(\.absoluteString)) ?? []
        guard !urls.isEmpty, !profile.accessKey.isEmpty else { return }
        let label = [profile.displayName, libraryName]
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
        let value = Value(
            urls: urls,
            accessKey: profile.accessKey,
            targetIdentity: profile.targetIdentity,
            targetDeviceId: profile.targetDeviceId,
            label: label
        )
        guard let data = try? JSONEncoder().encode(value) else { return }
        UserDefaults(suiteName: appGroupID)?.set(data, forKey: storageKey)
    }

    static func clear() {
        UserDefaults(suiteName: appGroupID)?.removeObject(forKey: storageKey)
    }
}

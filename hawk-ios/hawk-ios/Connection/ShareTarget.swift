import Foundation

/// 共享给分享扩展的目标（App Group UserDefaults）。
/// 扩展按候选地址与 AccessKey 直接访问 daemon 的局域网 HTTP API。
nonisolated enum ShareTarget {
    static let appGroupID = "group.com.soryu-ryouji.hawk"
    static let storageKey = "shareTarget.v3"

    struct Value: Codable, Sendable {
        var urls: [String]
        var accessKey: String
        /// 展示名：连接名 · 素材库名
        var label: String
    }

    static func save(profile: ConnectionProfile, libraryName: String) {
        let urls = (try? profile.candidateBaseURLs().map(\.absoluteString)) ?? []
        guard !urls.isEmpty, !profile.accessKey.isEmpty else {
            return
        }
        let label = [profile.displayName, libraryName]
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
        let value = Value(
            urls: urls,
            accessKey: profile.accessKey,
            label: label
        )
        guard let data = try? JSONEncoder().encode(value) else { return }
        UserDefaults(suiteName: appGroupID)?.set(data, forKey: storageKey)
    }

    static func clear() {
        UserDefaults(suiteName: appGroupID)?.removeObject(forKey: storageKey)
    }
}

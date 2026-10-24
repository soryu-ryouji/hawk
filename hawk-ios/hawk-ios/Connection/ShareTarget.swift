import Foundation

/// 共享给分享扩展的局域网目标（App Group UserDefaults）。
///
/// 分享扩展是独立进程，读不到主 App 的 Keychain；这里只放「当前局域网素材库」的候选地址与访问密钥，
/// 主 App 在局域网连接成功后写入、显式断开/退出连接时清除。视频/远端库不在 M1 范围内。
/// 与 `hawk-ios-share/ShareTargetStore.swift` 的 group/key/字段一一对应，改动需同步。
nonisolated enum ShareTarget {
    static let appGroupID = "group.com.soryu-ryouji.hawk"
    static let storageKey = "shareTarget.v1"

    struct Value: Codable, Sendable {
        /// 同一目标的所有候选地址（按优选顺序，扩展逐个探测）
        var urls: [String]
        /// 局域网访问密钥（桌面端配对二维码里的 token）
        var token: String
        /// 展示名：连接名 · 素材库名
        var label: String
    }

    static func save(profile: ConnectionProfile, libraryName: String) {
        guard profile.kind == .lan else { return }
        var seen = Set<String>()
        let urls = ([profile.url] + (profile.urls ?? []))
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty && seen.insert($0).inserted }
        guard !urls.isEmpty, !profile.token.isEmpty else { return }
        let label = [profile.displayName, libraryName]
            .filter { !$0.isEmpty }
            .joined(separator: " · ")
        guard
            let data = try? JSONEncoder().encode(
                Value(urls: urls, token: profile.token, label: label))
        else { return }
        UserDefaults(suiteName: appGroupID)?.set(data, forKey: storageKey)
    }

    static func clear() {
        UserDefaults(suiteName: appGroupID)?.removeObject(forKey: storageKey)
    }
}

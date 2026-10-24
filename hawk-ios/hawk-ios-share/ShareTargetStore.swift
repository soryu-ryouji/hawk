import Foundation

/// 读主 App 通过 App Group 写入的局域网目标。
/// 与主工程 `Connection/ShareTarget.swift` 的 group/key/字段一一对应，改动需同步。
nonisolated enum ShareTargetStore {
    static let appGroupID = "group.com.soryu-ryouji.hawk"
    static let storageKey = "shareTarget.v1"

    struct Target: Codable, Sendable {
        /// 同一目标的候选地址（按优选顺序，扩展逐个探测）
        var urls: [String]
        /// 局域网访问密钥（桌面端配对二维码里的 token）
        var token: String
        /// 展示名：连接名 · 素材库名
        var label: String
    }

    /// 主 App 未连接局域网素材库时返回 nil（分享扩展此时只能提示去 App 里连接）
    static func load() -> Target? {
        guard let data = UserDefaults(suiteName: appGroupID)?.data(forKey: storageKey) else {
            return nil
        }
        return try? JSONDecoder().decode(Target.self, from: data)
    }
}

import Foundation

/// 读主 App 通过 App Group 写入的目标；字段与 Connection/ShareTarget.swift 一致。
nonisolated enum ShareTargetStore {
    static let appGroupID = "group.com.soryu-ryouji.hawk"
    static let storageKey = "shareTarget.v3"

    struct Target: Codable, Sendable {
        var urls: [String]
        var accessKey: String
        var label: String
    }

    static func load() -> Target? {
        guard let data = UserDefaults(suiteName: appGroupID)?.data(forKey: storageKey) else {
            return nil
        }
        return try? JSONDecoder().decode(Target.self, from: data)
    }
}

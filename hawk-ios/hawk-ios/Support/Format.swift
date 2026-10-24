import Foundation

nonisolated enum Format {
    /// 客户端界面为中文，日期不跟随系统语言（避免「Oct 24, 2026 at 22:12」混排）
    private static let locale = Locale(identifier: "zh_Hans_CN")

    static func bytes(_ value: Int64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        formatter.allowedUnits = [.useKB, .useMB, .useGB]
        return formatter.string(fromByteCount: value)
    }

    static func date(milliseconds: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(milliseconds) / 1000)
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.dateStyle = .medium
        formatter.timeStyle = .short
        return formatter.string(from: date)
    }

    static func dimensions(width: Int, height: Int) -> String {
        "\(width) × \(height)"
    }
}

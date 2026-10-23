import Foundation

nonisolated enum Format {
    static func bytes(_ value: Int64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        formatter.allowedUnits = [.useKB, .useMB, .useGB]
        return formatter.string(fromByteCount: value)
    }

    static func date(milliseconds: Int64) -> String {
        let date = Date(timeIntervalSince1970: Double(milliseconds) / 1000)
        return date.formatted(date: .abbreviated, time: .shortened)
    }

    static func dimensions(width: Int, height: Int) -> String {
        "\(width) × \(height)"
    }
}

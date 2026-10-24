import Foundation

/// 解锁票据：按「维度:名称」持有，随请求以 `X-Hawk-Unlock` 头（逗号分隔）或媒体 URL 的
/// `?unlock=` 参数附带。票据只存在本客户端内存中，丢弃即「锁定回去」。
/// 服务端语义见 hawk-daemon/src/core/locks.rs。
nonisolated final class UnlockTicketStore: @unchecked Sendable {
    private let lock = NSLock()
    private var tickets: [String: String] = [:]

    private static func key(_ dimension: LockDimension, _ name: String) -> String {
        "\(dimension.rawValue):\(name)"
    }

    func add(_ token: String, for target: LockTarget) {
        lock.withLock {
            tickets[Self.key(target.dimension, target.name)] = token
        }
    }

    func contains(_ target: LockTarget) -> Bool {
        lock.withLock { tickets[Self.key(target.dimension, target.name)] != nil }
    }

    func drop(_ target: LockTarget) {
        _ = lock.withLock {
            tickets.removeValue(forKey: Self.key(target.dimension, target.name))
        }
    }

    func dropAll() {
        lock.withLock { tickets.removeAll() }
    }

    /// 请求头取值：多票逗号分隔；无票返回 nil（不发送空头）。
    func headerValue() -> String? {
        lock.withLock {
            tickets.isEmpty ? nil : tickets.values.joined(separator: ",")
        }
    }
}

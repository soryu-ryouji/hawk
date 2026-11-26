import Foundation

@_silgen_name("hawk_transport_connect_access")
nonisolated private func transportConnectAccess(
    _ configDir: UnsafePointer<CChar>?,
    _ serverUrl: UnsafePointer<CChar>?,
    _ targetDeviceId: UnsafePointer<CChar>?,
    _ targetIdentity: UnsafePointer<CChar>?,
    _ accessKey: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>?

@_silgen_name("hawk_transport_disconnect")
nonisolated private func transportDisconnect(_ sessionId: UnsafePointer<CChar>?)
    -> UnsafeMutablePointer<CChar>?

@_silgen_name("hawk_transport_free_string")
nonisolated private func transportFreeString(_ pointer: UnsafeMutableRawPointer?)

nonisolated struct RustTransportError: LocalizedError, Sendable {
    let message: String
    var errorDescription: String? { message }
}

nonisolated struct NativeConnection: Decodable, Sendable {
    let sessionId: String
    let apiBase: String
    let credential: String
    let role: String
    let targetDeviceId: String

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case apiBase = "api_base"
        case credential, role
        case targetDeviceId = "target_device_id"
    }
}

/// 持有原生会话直到主 App 或分享扩展完成清理；重复 close 是安全的。
nonisolated final class NativeTransportSession: @unchecked Sendable {
    let connection: NativeConnection
    private let lock = NSLock()
    private var closed = false
    private var closeTask: Task<Void, Never>?

    init(connection: NativeConnection) {
        self.connection = connection
    }

    func close() {
        lock.withLock {
            guard !closed else { return }
            closed = true
            let sessionId = connection.sessionId
            // disconnect 会等待控制面响应，不能阻塞 UI/deinit 所在线程。
            closeTask = Task.detached(priority: .utility) {
                RustTransport.disconnect(sessionId: sessionId)
            }
        }
    }

    /// Share Extension 退出前必须等待清理，否则进程回收会丢失 rendezvous 取消。
    func waitForClose() async {
        close()
        let task = lock.withLock { closeTask }
        await task?.value
    }

    deinit {
        close()
    }
}

nonisolated enum RustTransport {
    private struct Response: Decodable {
        let sessionId: String?
        let apiBase: String?
        let credential: String?
        let role: String?
        let targetDeviceId: String?
        let error: String?

        enum CodingKeys: String, CodingKey {
            case sessionId = "session_id"
            case apiBase = "api_base"
            case credential, role, error
            case targetDeviceId = "target_device_id"
        }
    }

    static func connect(
        configDir: String,
        serverUrl: String,
        targetDeviceId: String,
        targetIdentity: String,
        accessKey: String
    ) async throws -> NativeConnection {
        // C ABI 会同步等待 rendezvous 与隧道登录，不能占用 MainActor/协作线程。
        try await Task.detached(priority: .userInitiated) {
            guard
                let raw = transportConnectAccess(
                    configDir, serverUrl, targetDeviceId, targetIdentity, accessKey)
            else {
                throw RustTransportError(message: "原生传输初始化失败")
            }
            defer { transportFreeString(UnsafeMutableRawPointer(raw)) }
            let text = String(cString: raw)
            guard let data = text.data(using: .utf8) else {
                throw RustTransportError(message: "原生传输响应不是有效 UTF-8")
            }
            return try decodeConnection(data)
        }.value
    }

    static func decodeConnection(_ data: Data) throws -> NativeConnection {
        let result: Response
        do {
            result = try JSONDecoder().decode(Response.self, from: data)
        } catch {
            throw RustTransportError(message: "原生传输响应无法解析")
        }
        if let error = result.error, !error.isEmpty {
            throw RustTransportError(message: error)
        }
        guard let sessionId = result.sessionId,
            let apiBase = result.apiBase,
            let credential = result.credential,
            let role = result.role,
            let targetDeviceId = result.targetDeviceId
        else {
            throw RustTransportError(message: "原生传输响应缺少会话字段")
        }
        return NativeConnection(
            sessionId: sessionId, apiBase: apiBase, credential: credential, role: role,
            targetDeviceId: targetDeviceId)
    }

    static func disconnect(sessionId: String) {
        guard let raw = transportDisconnect(sessionId) else { return }
        transportFreeString(UnsafeMutableRawPointer(raw))
    }
}

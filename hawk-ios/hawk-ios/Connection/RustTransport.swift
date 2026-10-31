import Foundation

@_silgen_name("hawk_transport_connect_access")
private func transportConnectAccess(
    _ configDir: UnsafePointer<CChar>?,
    _ serverUrl: UnsafePointer<CChar>?,
    _ targetDeviceId: UnsafePointer<CChar>?,
    _ targetIdentity: UnsafePointer<CChar>?,
    _ accessKey: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>?

@_silgen_name("hawk_transport_disconnect")
private func transportDisconnect(_ sessionId: UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?

@_silgen_name("hawk_transport_free_string")
private func transportFreeString(_ pointer: UnsafeMutableRawPointer?)

struct NativeConnection: Decodable, Sendable {
    let sessionId: String
    let apiBase: String
    let credential: String
    let role: String
    let targetDeviceId: String
    let error: String?

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case apiBase = "api_base"
        case credential, role, error
        case targetDeviceId = "target_device_id"
    }
}

enum RustTransport {
    static func connect(
        configDir: String,
        serverUrl: String,
        targetDeviceId: String,
        targetIdentity: String,
        accessKey: String
    ) throws -> NativeConnection {
        guard let raw = transportConnectAccess(configDir, serverUrl, targetDeviceId, targetIdentity, accessKey) else {
            throw ConnectionError.message("原生传输初始化失败")
        }
        defer { transportFreeString(UnsafeMutableRawPointer(raw)) }
        let text = String(cString: raw)
        guard let data = text.data(using: .utf8) else {
            throw ConnectionError.message("原生传输响应不是有效 UTF-8")
        }
        let result = try JSONDecoder().decode(NativeConnection.self, from: data)
        if let error = result.error, !error.isEmpty {
            throw ConnectionError.message(error)
        }
        return result
    }

    static func disconnect(sessionId: String) {
        guard let raw = transportDisconnect(sessionId) else { return }
        transportFreeString(UnsafeMutableRawPointer(raw))
    }
}

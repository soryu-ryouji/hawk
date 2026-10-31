import Foundation

/// V2 统一配对：AccessKey + target_identity + LAN/WAN endpoints，不携带 tenant key。
nonisolated struct PairingPayload: Decodable, Sendable {
    struct Wan: Decodable, Sendable {
        let serverUrl: String

        enum CodingKeys: String, CodingKey {
            case serverUrl = "server_url"
        }
    }

    let v: Int
    let targetLibraryId: String?
    let targetDeviceId: String?
    let targetIdentity: String = ""
    let tlsFingerprint: String?
    let accessKey: String = ""
    let lanEndpoints: [String]
    let wan: Wan?

    enum CodingKeys: String, CodingKey {
        case v, wan
        case targetLibraryId = "target_library_id"
        case targetDeviceId = "target_device_id"
        case targetIdentity = "target_identity"
        case tlsFingerprint = "tls_fingerprint"
        case accessKey = "access_key"
        case lanEndpoints = "lan_endpoints"
    }

    static func decode(_ code: String) throws -> PairingPayload {
        guard let data = code.data(using: .utf8) else {
            throw ConnectionError.message("二维码内容无法读取")
        }
        let payload: PairingPayload
        do {
            payload = try JSONDecoder().decode(PairingPayload.self, from: data)
        } catch {
            throw ConnectionError.message("这不是 hawk 的配对二维码")
        }
        guard payload.v == 2 else {
            throw ConnectionError.message("二维码版本不支持，请升级 hawk iOS 客户端")
        }
        return payload
    }

    func profile() throws -> ConnectionProfile {
        guard !targetIdentity.isEmpty, !accessKey.isEmpty else {
            throw ConnectionError.message("配对二维码缺少目标身份或 AccessKey")
        }
        let candidates = lanEndpoints
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        var seen = Set<String>()
        let unique = candidates.filter { seen.insert($0.lowercased()).inserted }
        let serverUrl = wan?.serverUrl.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let kind: ConnectionKind = serverUrl.isEmpty ? .lan : .remote
        let url = serverUrl.isEmpty ? unique.first ?? "" : serverUrl
        guard !url.isEmpty else {
            throw ConnectionError.message("配对二维码缺少访问地址")
        }
        return ConnectionProfile(
            kind: kind,
            url: url,
            urls: unique,
            token: accessKey,
            appKey: accessKey,
            targetIdentity: targetIdentity,
            targetLibraryId: targetLibraryId ?? "",
            targetDeviceId: targetDeviceId ?? "",
            tlsFingerprint: tlsFingerprint ?? ""
        )
    }
}

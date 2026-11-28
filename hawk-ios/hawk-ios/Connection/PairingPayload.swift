import Foundation

/// V3 局域网配对：AccessKey + 至少一个 HTTP endpoint，不携带任何目标身份。
nonisolated struct PairingPayload: Decodable, Sendable {
    let v: Int
    let targetLibraryId: String?
    let accessKey: String?
    let lanEndpoints: [String]?

    enum CodingKeys: String, CodingKey {
        case v
        case targetLibraryId = "target_library_id"
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
        guard payload.v == 3 else {
            throw ConnectionError.message("二维码版本不支持，请升级 hawk iOS 客户端")
        }
        return payload
    }

    func profile() throws -> ConnectionProfile {
        let key = accessKey?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !key.isEmpty else {
            throw ConnectionError.message("配对二维码缺少 AccessKey")
        }
        let candidates =
            (lanEndpoints ?? [])
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        var seen = Set<String>()
        let unique = candidates.filter { seen.insert($0.lowercased()).inserted }
        guard let url = unique.first else {
            throw ConnectionError.message("配对二维码缺少局域网地址")
        }
        return ConnectionProfile(kind: .lan, url: url, urls: unique, accessKey: key)
    }
}

import Foundation

/// 桌面端生成的配对二维码载荷（设计文档「鉴权与配对」）。
/// 局域网：{ "v": 1, "kind": "lan", "url": "...", "urls": ["..."], "token": "..." }
/// 广域网：{ "v": 1, "kind": "remote", "url": "...", "service_key": "...", "key": "..." }
nonisolated struct PairingPayload: Decodable, Sendable {
    let v: Int
    let kind: String
    let url: String
    let urls: [String]?
    let token: String?
    let serviceKey: String?
    let key: String?

    enum CodingKeys: String, CodingKey {
        case v, kind, url, urls, token, key
        case serviceKey = "service_key"
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
        guard payload.v == 1 else {
            throw ConnectionError.message("二维码版本不支持，请升级 hawk iOS 客户端")
        }
        return payload
    }

    func profile() throws -> ConnectionProfile {
        switch kind {
        case "lan":
            guard let token, !token.isEmpty else {
                throw ConnectionError.message("配对二维码缺少访问 token")
            }
            let candidates = ([url] + (urls ?? []))
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .filter { !$0.isEmpty }
            var seen = Set<String>()
            let unique = candidates.filter { seen.insert($0.lowercased()).inserted }
            guard let primary = unique.first else {
                throw ConnectionError.message("配对二维码缺少访问地址")
            }
            return ConnectionProfile(kind: .lan, url: primary, urls: unique, token: token)
        case "remote":
            guard let serviceKey, let key, !serviceKey.isEmpty, !key.isEmpty else {
                throw ConnectionError.message("配对二维码缺少访问密钥")
            }
            return ConnectionProfile(kind: .remote, url: url, serviceKey: serviceKey, appKey: key)
        default:
            throw ConnectionError.message("二维码类型不支持")
        }
    }
}

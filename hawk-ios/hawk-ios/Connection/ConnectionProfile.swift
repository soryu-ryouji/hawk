import Foundation

nonisolated enum ConnectionKind: String, Codable, Sendable {
    case lan
    case remote

    var title: String {
        switch self {
        case .lan: "局域网"
        case .remote: "广域网"
        }
    }

    var defaultScheme: String {
        switch self {
        case .lan: "http"
        case .remote: "https"
        }
    }
}

/// 一条连接配置。V2 配对由 targetIdentity + accessKey 定位并认证目标；
/// 没有 targetIdentity 的手工 LAN 先通过 Peer 公开信息解析身份。
nonisolated struct ConnectionProfile: Codable, Sendable, Identifiable, Hashable {
    var id: UUID
    var kind: ConnectionKind
    var name: String
    var url: String
    /// 同一目标的候选地址（WAN 服务地址与 LAN rendezvous 地址可同时存在）。
    var urls: [String]?
    var accessKey: String
    /// V2 统一配对：目标 iroh 公钥与库身份。
    var targetIdentity: String
    var targetLibraryId: String
    var targetDeviceId: String
    var tlsFingerprint: String

    init(
        id: UUID = UUID(),
        kind: ConnectionKind,
        name: String = "",
        url: String,
        urls: [String]? = nil,
        accessKey: String = "",
        targetIdentity: String = "",
        targetLibraryId: String = "",
        targetDeviceId: String = "",
        tlsFingerprint: String = ""
    ) {
        self.id = id
        self.kind = kind
        self.name = name
        self.url = url
        self.urls = urls
        self.accessKey = accessKey
        self.targetIdentity = targetIdentity
        self.targetLibraryId = targetLibraryId
        self.targetDeviceId = targetDeviceId
        self.tlsFingerprint = tlsFingerprint
    }

    var displayName: String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            return trimmed
        }
        return (try? baseURL())?.host ?? url
    }

    /// 把 Peer 公开配对信息固化进连接，后续启动与分享扩展可直接使用原生通道。
    func resolving(with info: LanConnectionInfo, at baseURL: URL) -> ConnectionProfile {
        var resolved = self
        resolved.url = baseURL.absoluteString
        resolved.targetIdentity = info.targetIdentity
        resolved.targetDeviceId = info.targetDeviceId ?? ""
        resolved.targetLibraryId = info.targetLibraryId
        resolved.tlsFingerprint = info.tlsFingerprint ?? ""
        if resolved.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            resolved.name = info.libraryName
        }
        return resolved
    }

    /// WAN 优先，失败后尝试配对载荷中的 LAN 候选地址。
    func candidateBaseURLs() throws -> [URL] {
        let candidates = ([url] + (urls ?? []))
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        var seen = Set<String>()
        let unique = candidates.filter { seen.insert($0.lowercased()).inserted }
        guard !unique.isEmpty else {
            throw ConnectionError.message("请填写服务地址")
        }
        return try unique.map { try normalizedBaseURL($0) }
    }

    /// 规范化用户输入的地址：补 scheme、去掉路径尾部斜杠与查询参数。
    func baseURL() throws -> URL {
        try normalizedBaseURL(url)
    }

    private func normalizedBaseURL(_ rawValue: String) throws -> URL {
        var raw = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty else {
            throw ConnectionError.message("请填写服务地址")
        }
        if !raw.contains("://") {
            raw = "\(kind.defaultScheme)://\(raw)"
        }
        guard
            var components = URLComponents(string: raw),
            let scheme = components.scheme?.lowercased(),
            scheme == "http" || scheme == "https",
            let host = components.host,
            !host.isEmpty
        else {
            throw ConnectionError.message(
                "服务地址无效，请填写形如 192.168.1.5:27375 或 https://hawk.example.com 的地址")
        }
        components.query = nil
        components.fragment = nil
        while components.path.hasSuffix("/") {
            components.path.removeLast()
        }
        guard let url = components.url else {
            throw ConnectionError.message("服务地址无效")
        }
        return url
    }
}

nonisolated struct ConnectionBookmark: Codable, Sendable {
    var profiles: [ConnectionProfile]
    var activeID: UUID?
}

nonisolated enum ConnectionError: LocalizedError, Sendable {
    case message(String)

    var errorDescription: String? {
        switch self {
        case .message(let text): text
        }
    }
}

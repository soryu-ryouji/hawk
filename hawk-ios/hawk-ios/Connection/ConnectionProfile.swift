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

/// 一条连接配置。局域网用 url + token；广域网用 url + serviceKey + appKey。
nonisolated struct ConnectionProfile: Codable, Sendable, Identifiable, Hashable {
    var id: UUID
    var kind: ConnectionKind
    var name: String
    var url: String
    var token: String
    var serviceKey: String
    var appKey: String

    init(
        id: UUID = UUID(),
        kind: ConnectionKind,
        name: String = "",
        url: String,
        token: String = "",
        serviceKey: String = "",
        appKey: String = ""
    ) {
        self.id = id
        self.kind = kind
        self.name = name
        self.url = url
        self.token = token
        self.serviceKey = serviceKey
        self.appKey = appKey
    }

    var displayName: String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            return trimmed
        }
        return (try? baseURL())?.host ?? url
    }

    /// 规范化用户输入的地址：补 scheme、去掉路径尾部斜杠与查询参数。
    func baseURL() throws -> URL {
        var raw = url.trimmingCharacters(in: .whitespacesAndNewlines)
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
                "服务地址无效，请填写形如 192.168.1.5:27372 或 https://hawk.example.com 的地址")
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

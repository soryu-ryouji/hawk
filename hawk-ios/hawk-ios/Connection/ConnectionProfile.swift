import Foundation

nonisolated enum ConnectionKind: String, Codable, Sendable {
    case lan

    var title: String {
        switch self {
        case .lan: "局域网"
        }
    }

    var defaultScheme: String {
        switch self {
        case .lan: "http"
        }
    }
}

/// 一条纯局域网直连配置：daemon 地址 + AccessKey。
nonisolated struct ConnectionProfile: Codable, Sendable, Identifiable, Hashable {
    var id: UUID
    var kind: ConnectionKind
    var name: String
    /// 首个候选地址，同时作为列表与详情展示的地址。
    var url: String
    /// 配对载荷中的其余候选地址。
    var urls: [String]?
    var accessKey: String

    init(
        id: UUID = UUID(),
        kind: ConnectionKind,
        name: String = "",
        url: String,
        urls: [String]? = nil,
        accessKey: String = ""
    ) {
        self.id = id
        self.kind = kind
        self.name = name
        self.url = url
        self.urls = urls
        self.accessKey = accessKey
    }

    var displayName: String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            return trimmed
        }
        return (try? baseURL())?.host ?? url
    }

    /// 用目标公开信息补全地址与展示名，不保存任何目标身份。
    func resolving(with info: LanConnectionInfo, at baseURL: URL) -> ConnectionProfile {
        var resolved = self
        resolved.url = baseURL.absoluteString
        if resolved.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            resolved.name = info.libraryName
        }
        return resolved
    }

    /// 逐个尝试候选地址，首个为 `url`，其余来自配对载荷。
    func candidateBaseURLs() throws -> [URL] {
        let candidates = ([url] + (urls ?? []))
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        var seen = Set<String>()
        let unique = candidates.filter { seen.insert($0.lowercased()).inserted }
        guard !unique.isEmpty else {
            throw ConnectionError.message("请填写局域网地址")
        }
        return try unique.map { try normalizedBaseURL($0) }
    }

    /// 规范化用户输入的地址：补 http scheme、去掉路径尾部斜杠与查询参数。
    func baseURL() throws -> URL {
        try normalizedBaseURL(url)
    }

    private func normalizedBaseURL(_ rawValue: String) throws -> URL {
        var raw = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty else {
            throw ConnectionError.message("请填写局域网地址")
        }
        if !raw.contains("://") {
            raw = "\(kind.defaultScheme)://\(raw)"
        }
        guard
            var components = URLComponents(string: raw),
            let scheme = components.scheme?.lowercased(),
            scheme == "http",
            let host = components.host,
            !host.isEmpty
        else {
            throw ConnectionError.message(
                "局域网地址无效，请填写形如 192.168.1.5:27375 的地址")
        }
        components.query = nil
        components.fragment = nil
        while components.path.hasSuffix("/") {
            components.path.removeLast()
        }
        guard let url = components.url else {
            throw ConnectionError.message("局域网地址无效")
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

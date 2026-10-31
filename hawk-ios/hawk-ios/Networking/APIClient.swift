import Foundation

nonisolated struct APIError: LocalizedError, Sendable, Equatable {
    let code: String
    let message: String
    let status: Int

    var errorDescription: String? { message }
    var isUnauthorized: Bool { status == 401 || code == "UNAUTHORIZED" }
    var isNotReady: Bool { code == "NOT_READY" || status == 503 }
    var isLocked: Bool { code == "LOCKED" }
    var isThrottled: Bool { code == "THROTTLED" || status == 429 }
    var isReadOnly: Bool { code == "READ_ONLY" }
    var isOffline: Bool { status == 0 }
}

/// 会话凭证：token 在 401 时可由 refresher 重换（广域网 bt 短时效；
/// 局域网凭证长期有效，refresher 为空即直接抛错）。
nonisolated final class SessionCredentials: @unchecked Sendable {
    private let lock = NSLock()
    private var token: String
    private var refreshTask: Task<Void, any Error>?
    private let refresh: (@Sendable () async throws -> String)?

    init(token: String, refresh: (@Sendable () async throws -> String)? = nil) {
        self.token = token
        self.refresh = refresh
    }

    func current() -> String {
        lock.withLock { token }
    }

    /// 并发安全的一次性重换：多个请求同时 401 只触发一次登录。
    func renew() async throws {
        guard let refresh else {
            throw APIError(code: "UNAUTHORIZED", message: "访问凭证已失效，请重新连接", status: 401)
        }
        let task: Task<Void, any Error> = lock.withLock {
            if let existing = refreshTask {
                return existing
            }
            let created = Task { [weak self] in
                defer { self?.clearRefreshTask() }
                let value = try await refresh()
                self?.set(value)
            }
            refreshTask = created
            return created
        }
        try await task.value
    }

    private func set(_ value: String) {
        lock.withLock { token = value }
    }

    private func clearRefreshTask() {
        lock.withLock { refreshTask = nil }
    }
}

nonisolated final class APIClient: @unchecked Sendable {
    let baseURL: URL
    let credentials: SessionCredentials
    let tickets: UnlockTicketStore
    private let session: URLSession
    var sessionToken: String { credentials.current() }
    /// SSE 长连接专用会话：必须由客户端持有，随请求销毁会让流被取消
    private let streamSession: URLSession

    init(
        baseURL: URL,
        credentials: SessionCredentials,
        tickets: UnlockTicketStore = UnlockTicketStore()
    ) {
        self.baseURL = baseURL
        self.credentials = credentials
        self.tickets = tickets
        let configuration = URLSessionConfiguration.default
        configuration.timeoutIntervalForRequest = 30
        configuration.waitsForConnectivity = false
        self.session = URLSession(configuration: configuration)

        let streaming = URLSessionConfiguration.default
        streaming.timeoutIntervalForRequest = 3600
        streaming.timeoutIntervalForResource = .infinity
        streaming.requestCachePolicy = .reloadIgnoringLocalCacheData
        self.streamSession = URLSession(configuration: streaming)
    }

    // MARK: - Typed requests

    func get<Response: Decodable & Sendable>(
        _ path: String,
        query: [URLQueryItem] = [],
        timeout: TimeInterval = 30
    ) async throws -> Response {
        try await send("GET", path, query: query, timeout: timeout)
    }

    func post<Response: Decodable & Sendable, Body: Encodable & Sendable>(
        _ path: String,
        _ body: Body,
        timeout: TimeInterval = 60
    ) async throws -> Response {
        try await send("POST", path, body: try encode(body), timeout: timeout)
    }

    func post<Body: Encodable & Sendable>(
        _ path: String,
        _ body: Body,
        timeout: TimeInterval = 60
    ) async throws {
        let data = try await perform("POST", path, body: try encode(body), timeout: timeout)
        try checkStatus(data)
    }

    func put<Body: Encodable & Sendable>(
        _ path: String,
        _ body: Body,
        timeout: TimeInterval = 30
    ) async throws {
        let data = try await perform("PUT", path, body: try encode(body), timeout: timeout)
        try checkStatus(data)
    }

    func delete(_ path: String, query: [URLQueryItem] = [], timeout: TimeInterval = 30) async throws
    {
        let data = try await perform("DELETE", path, query: query, body: nil, timeout: timeout)
        try checkStatus(data)
    }

    // MARK: - Media

    /// 缩略图 / 原图地址：使用 Session Cookie，解锁票据只表达锁可见范围。
    func mediaURL(path: String, id: String) -> URL? {
        var query = [URLQueryItem(name: "id", value: id)]
        if let unlock = tickets.headerValue() {
            query.append(URLQueryItem(name: "unlock", value: unlock))
        }
        return makeURL(path: path, query: query)
    }

    func thumbnailURL(id: String) -> URL? {
        mediaURL(path: "/api/v1/item/thumbnail", id: id)
    }

    func fileURL(id: String) -> URL? {
        mediaURL(path: "/api/v1/item/file", id: id)
    }

    func fetchFileData(id: String) async throws -> Data {
        guard let url = fileURL(id: id) else {
            throw APIError(code: "INVALID_URL", message: "图片地址无效", status: 0)
        }
        var request = URLRequest(url: url)
        request.timeoutInterval = 120
        request.setValue("hawk_session=\(credentials.current())", forHTTPHeaderField: "Cookie")
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw APIError(code: "NETWORK", message: "网络响应无效", status: 0)
        }
        guard (200..<300).contains(http.statusCode) else {
            throw APIError(
                code: "HTTP_\(http.statusCode)", message: "下载原图失败（HTTP \(http.statusCode)）",
                status: http.statusCode)
        }
        return data
    }

    // MARK: - Event stream

    /// SSE 订阅请求：Bearer Session + Cookie，不在 URL 中携带长期凭证。
    func eventRequest() -> URLRequest? {
        guard
            let url = makeURL(
                path: "/api/v1/events",
                query: [
                    URLQueryItem(name: "unlock", value: tickets.headerValue() ?? ""),
                ]
            )
        else { return nil }
        var request = URLRequest(url: url)
        request.setValue("hawk_session=\(credentials.current())", forHTTPHeaderField: "Cookie")
        request.timeoutInterval = 3600
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.setValue("no-cache", forHTTPHeaderField: "Cache-Control")
        return request
    }

    func streamEvents() async throws -> URLSession.AsyncBytes {
        guard let request = eventRequest() else { throw APIError.invalidURL }
        let (bytes, response) = try await streamSession.bytes(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw APIError(code: "NETWORK", message: "网络响应无效", status: 0)
        }
        guard http.statusCode == 200 else {
            throw APIError(
                code: "HTTP_\(http.statusCode)",
                message: "订阅素材库变更失败（HTTP \(http.statusCode)）",
                status: http.statusCode
            )
        }
        return bytes
    }

    // MARK: - Transport

    private func perform(
        _ method: String,
        _ path: String,
        query: [URLQueryItem] = [],
        body: Data?,
        timeout: TimeInterval,
        allowsRenewal: Bool = true
    ) async throws -> Data {
        guard let url = makeURL(path: path, query: query) else {
            throw APIError(code: "INVALID_URL", message: "服务地址无效", status: 0)
        }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = timeout
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let token = credentials.current()
        if !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        if let unlock = tickets.headerValue() {
            request.setValue(unlock, forHTTPHeaderField: "X-Hawk-Unlock")
        }
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }

        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch let error as URLError where error.code == .cancelled {
            throw CancellationError()
        } catch let error as URLError {
            throw APIError(code: "NETWORK", message: Self.networkMessage(for: error), status: 0)
        } catch {
            throw APIError(code: "NETWORK", message: "无法连接素材库服务", status: 0)
        }

        guard let http = response as? HTTPURLResponse else {
            throw APIError(code: "NETWORK", message: "网络响应无效", status: 0)
        }
        if http.statusCode == 401, allowsRenewal {
            // bt 过期：重换一次凭证后重放（局域网凭据不会 401，renew 会直接抛错）
            try await credentials.renew()
            return try await perform(
                method, path, query: query, body: body, timeout: timeout, allowsRenewal: false)
        }
        guard (200..<300).contains(http.statusCode) else {
            let envelope = try? decode(ErrorEnvelope.self, from: data)
            let payload = envelope?.error
            throw APIError(
                code: payload?.code ?? "HTTP_\(http.statusCode)",
                message: payload?.message ?? "请求失败（HTTP \(http.statusCode)）",
                status: http.statusCode
            )
        }
        return data
    }

    private func send<Response: Decodable & Sendable>(
        _ method: String,
        _ path: String,
        query: [URLQueryItem] = [],
        body: Data? = nil,
        timeout: TimeInterval = 30
    ) async throws -> Response {
        let data = try await perform(method, path, query: query, body: body, timeout: timeout)
        let envelope: Envelope<Response> = try decode(Envelope<Response>.self, from: data)
        if envelope.status == "error" {
            throw envelopeError(envelope.error)
        }
        guard let value = envelope.data else {
            throw APIError(code: "EMPTY_RESPONSE", message: "服务端返回为空", status: 200)
        }
        return value
    }

    private func checkStatus(_ data: Data) throws {
        let envelope: StatusEnvelope = try decode(StatusEnvelope.self, from: data)
        if envelope.status == "error" {
            throw envelopeError(envelope.error)
        }
    }

    nonisolated private static func networkMessage(for error: URLError) -> String {
        switch error.code {
        case .notConnectedToInternet:
            return "设备当前没有网络连接"
        case .timedOut:
            return "连接素材库超时，请检查服务地址与网络"
        case .cannotFindHost, .cannotConnectToHost, .dnsLookupFailed:
            return "无法连接素材库服务：请确认地址、端口，以及系统设置中的本地网络权限"
        case .appTransportSecurityRequiresSecureConnection:
            return "系统阻止了明文 HTTP 连接：局域网请填写 IP 地址，广域网请使用 https"
        case .secureConnectionFailed, .serverCertificateUntrusted:
            return "服务端证书校验失败，请检查 HTTPS 配置"
        default:
            return "无法连接素材库服务（\(error.localizedDescription)）"
        }
    }

    private func makeURL(path: String, query: [URLQueryItem]) -> URL? {
        guard var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false) else {
            return nil
        }
        var basePath = components.path
        while basePath.hasSuffix("/") {
            basePath.removeLast()
        }
        components.path = basePath + path
        if !query.isEmpty {
            components.queryItems = query
        }
        return components.url
    }

    private func encode<Body: Encodable>(_ body: Body) throws -> Data {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        do {
            return try encoder.encode(body)
        } catch {
            throw APIError(code: "ENCODE", message: "请求参数编码失败", status: 0)
        }
    }

    private func decode<Value: Decodable>(_ type: Value.Type, from data: Data) throws -> Value {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        do {
            return try decoder.decode(type, from: data)
        } catch {
            throw APIError(code: "DECODE", message: "无法解析服务端响应", status: 0)
        }
    }

    private func envelopeError(_ error: ErrorPayload?) -> APIError {
        APIError(
            code: error?.code ?? "INTERNAL",
            message: error?.message ?? "请求失败",
            status: 200
        )
    }
}

nonisolated private struct Envelope<Payload: Decodable & Sendable>: Decodable, Sendable {
    let status: String
    let data: Payload?
    let error: ErrorPayload?
}

nonisolated private struct StatusEnvelope: Decodable, Sendable {
    let status: String
    let error: ErrorPayload?
}

nonisolated private struct ErrorEnvelope: Decodable, Sendable {
    let error: ErrorPayload?
}

nonisolated private struct ErrorPayload: Decodable, Sendable {
    let code: String?
    let message: String?
}

import Foundation

nonisolated struct APIError: LocalizedError, Sendable, Equatable {
    let code: String
    let message: String
    let status: Int

    var errorDescription: String? { message }
    var isUnauthorized: Bool { status == 401 || code == "UNAUTHORIZED" }
    var isNotReady: Bool { code == "NOT_READY" || status == 503 }
}

nonisolated struct APIClient: Sendable {
    let baseURL: URL
    let token: String

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
        let data = try await perform("POST", path, body: try encode(body), timeout: timeout)
        let envelope = try decode(Envelope<Response>.self, from: data)
        if envelope.status == "error" {
            throw envelopeError(envelope.error, status: 200)
        }
        guard let value = envelope.data else {
            throw APIError(code: "EMPTY_RESPONSE", message: "服务端返回为空", status: 200)
        }
        return value
    }

    func delete(_ path: String, timeout: TimeInterval = 30) async throws {
        let data = try await perform("DELETE", path, body: nil, timeout: timeout)
        let envelope = try decode(StatusEnvelope.self, from: data)
        if envelope.status == "error" {
            throw envelopeError(envelope.error, status: 200)
        }
    }

    // MARK: - Media

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
        let (data, response) = try await URLSession.shared.data(for: request)
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

    // MARK: - Transport

    private func perform(
        _ method: String,
        _ path: String,
        query: [URLQueryItem] = [],
        body: Data?,
        timeout: TimeInterval
    ) async throws -> Data {
        guard let url = makeURL(path: path, query: query) else {
            throw APIError(code: "INVALID_URL", message: "服务地址无效", status: 0)
        }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = timeout
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }

        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await URLSession.shared.data(for: request)
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
        let envelope = try decode(Envelope<Response>.self, from: data)
        if envelope.status == "error" {
            throw envelopeError(envelope.error, status: 200)
        }
        guard let value = envelope.data else {
            throw APIError(code: "EMPTY_RESPONSE", message: "服务端返回为空", status: 200)
        }
        return value
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

    private func mediaURL(path: String, id: String) -> URL? {
        makeURL(
            path: path,
            query: [
                URLQueryItem(name: "id", value: id),
                URLQueryItem(name: "token", value: token),
            ]
        )
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

    private func envelopeError(_ error: ErrorPayload?, status: Int) -> APIError {
        APIError(
            code: error?.code ?? "INTERNAL",
            message: error?.message ?? "请求失败",
            status: status
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

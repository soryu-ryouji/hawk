import Foundation

nonisolated struct ShareAPIError: LocalizedError, Sendable {
    let code: String
    let message: String
    let status: Int

    var errorDescription: String? { message }
    var isUnauthorized: Bool { status == 401 || code == "UNAUTHORIZED" }
    var isReadOnly: Bool { status == 403 || code == "READ_ONLY" }
    /// 目标路径已有同名文件（内容不同）
    var isDuplicate: Bool { code == "FILE_EXISTS" }
}

/// 分享扩展的最小 API 客户端：探活、读分类/标签、multipart 上传。
/// 不复用主 App 的 APIClient（那套带 SSE/解锁票据/连接重试，扩展只需要四个端点）。
nonisolated struct ShareAPIClient: Sendable {
    let baseURL: URL
    let token: String

    private struct Envelope<T: Decodable>: Decodable {
        let status: String
        let data: T?
        let error: ErrorPayload?
    }

    private struct ErrorPayload: Decodable {
        let code: String?
        let message: String?
    }

    private struct LibraryInfo: Decodable {
        let name: String
    }

    private struct Taxon: Decodable {
        let name: String
    }

    private struct UploadResult: Decodable {
        /// 内容已在库内、未重复写入
        let skipped: Bool
    }

    private struct Empty: Decodable {}

    func libraryName() async throws -> String {
        let info: LibraryInfo = try await get("/api/v1/library/info")
        return info.name
    }

    func categories() async throws -> [String] {
        let list: [Taxon] = try await get("/api/v1/category/list")
        return list.map(\.name)
    }

    func tags() async throws -> [String] {
        let list: [Taxon] = try await get("/api/v1/tag/list")
        return list.map(\.name)
    }

    /// 上传一张图片，并按分类/标签入库；返回 true 表示内容已存在被服务端跳过（视为成功）
    func upload(
        fileURL: URL,
        filename: String,
        categories: [String],
        tags: [String]
    ) async throws -> Bool {
        let boundary = "hawk-\(UUID().uuidString)"
        let bodyURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("hawk-upload-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: bodyURL) }
        try MultipartBody.write(
            to: bodyURL, boundary: boundary, fileURL: fileURL, filename: filename,
            categories: categories, tags: tags)

        var request = URLRequest(url: url("/api/v1/item/upload"))
        request.httpMethod = "POST"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue(
            "multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        request.timeoutInterval = 120
        let (data, response) = try await URLSession.shared.upload(for: request, fromFile: bodyURL)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        try Self.check(status: status, data: data)
        let envelope = try Self.decode(Envelope<UploadResult>.self, from: data)
        return envelope.data?.skipped ?? false
    }

    private func get<Value: Decodable>(_ path: String) async throws -> Value {
        var request = URLRequest(url: url(path))
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.timeoutInterval = 15
        let (data, response) = try await URLSession.shared.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        try Self.check(status: status, data: data)
        let envelope = try Self.decode(Envelope<Value>.self, from: data)
        guard envelope.status == "success", let value = envelope.data else {
            throw ShareAPIError(code: "INVALID_RESPONSE", message: "服务端返回错误", status: status)
        }
        return value
    }

    private func url(_ path: String) -> URL {
        URL(string: path, relativeTo: baseURL) ?? baseURL
    }

    private static func decode<Value: Decodable>(_ type: Value.Type, from data: Data) throws
        -> Value
    {
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch {
            throw ShareAPIError(code: "INVALID_RESPONSE", message: "服务端响应无法解析", status: 0)
        }
    }

    private static func check(status: Int, data: Data) throws {
        guard !(200..<300).contains(status) else { return }
        let envelope = try? JSONDecoder().decode(Envelope<Empty>.self, from: data)
        throw ShareAPIError(
            code: envelope?.error?.code ?? "",
            message: envelope?.error?.message ?? "服务端响应 \(status)",
            status: status)
    }
}

/// multipart 请求体：先落临时文件再上传，避免整张图与请求体同时驻留内存（扩展内存上限更紧）。
nonisolated enum MultipartBody {
    static func write(
        to bodyURL: URL,
        boundary: String,
        fileURL: URL,
        filename: String,
        categories: [String],
        tags: [String]
    ) throws {
        FileManager.default.createFile(atPath: bodyURL.path, contents: nil)
        let output = try FileHandle(forWritingTo: bodyURL)
        defer { try? output.close() }

        // 分类/标签按字段名重复出现（服务端按出现次数收集），空值不写
        for name in categories where !name.isEmpty {
            output.write(Data(textField(boundary: boundary, name: "categories", value: name).utf8))
        }
        for tag in tags where !tag.isEmpty {
            output.write(Data(textField(boundary: boundary, name: "tags", value: tag).utf8))
        }
        // 内容已在库内时跳过写入，避免重复收藏同一张图
        output.write(Data(textField(boundary: boundary, name: "skip_existing", value: "true").utf8))

        let safeName = sanitize(filename)
        let mime = mimeType(for: safeName)
        output.write(
            Data(
                "--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"\(safeName)\"\r\nContent-Type: \(mime)\r\n\r\n"
                    .utf8))

        let input = try FileHandle(forReadingFrom: fileURL)
        defer { try? input.close() }
        while let chunk = try input.read(upToCount: 1 << 20), !chunk.isEmpty {
            output.write(chunk)
        }
        output.write(Data("\r\n--\(boundary)--\r\n".utf8))
    }

    private static func textField(boundary: String, name: String, value: String) -> String {
        "--\(boundary)\r\nContent-Disposition: form-data; name=\"\(name)\"\r\n\r\n\(value)\r\n"
    }

    private static func sanitize(_ filename: String) -> String {
        filename
            .replacingOccurrences(of: "\"", with: "")
            .replacingOccurrences(of: "\r", with: "")
            .replacingOccurrences(of: "\n", with: "")
    }

    private static func mimeType(for filename: String) -> String {
        switch (filename as NSString).pathExtension.lowercased() {
        case "jpg", "jpeg": "image/jpeg"
        case "png": "image/png"
        case "gif": "image/gif"
        case "webp": "image/webp"
        default: "application/octet-stream"
        }
    }
}

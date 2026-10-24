import Foundation

/// SSE 订阅：`event: <名称>` + `data: <JSON>` 帧，空行分帧。
/// 服务端在消费落后（lagged）或总线关闭时直接断开，调用方需重连并全量对齐。
/// 契约见 hawk-daemon/src/api/events.rs 的 SseEvents 注册表。
nonisolated enum EventStream {
    static func events(api: APIClient) -> AsyncThrowingStream<ServerEvent, any Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let bytes = try await api.streamEvents()
                    var name = ""
                    var payload = ""
                    for try await line in bytes.lines {
                        if line.isEmpty {
                            if !name.isEmpty {
                                continuation.yield(
                                    ServerEvent(name: name, data: Data(payload.utf8)))
                            }
                            name = ""
                            payload = ""
                            continue
                        }
                        if line.hasPrefix(":") { continue }
                        if let value = field("event", in: line) {
                            name = value
                        } else if let value = field("data", in: line) {
                            payload += value
                        }
                    }
                    continuation.finish()
                } catch is CancellationError {
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private static func field(_ key: String, in line: String) -> String? {
        guard line.hasPrefix(key + ":") else { return nil }
        var value = String(line.dropFirst(key.count + 1))
        if value.hasPrefix(" ") { value.removeFirst() }
        return value
    }
}

extension APIError {
    static let invalidURL = APIError(code: "INVALID_URL", message: "服务地址无效", status: 0)
}

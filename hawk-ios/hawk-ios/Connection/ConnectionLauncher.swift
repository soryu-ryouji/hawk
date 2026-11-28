import Foundation

/// 纯局域网直连：逐个候选地址读取公开连接信息，校验协议后用 AccessKey 访问 daemon HTTP API。
nonisolated enum ConnectionLauncher {
    struct Outcome: Sendable {
        let api: APIClient
        let info: AppInfo
        let profile: ConnectionProfile
    }

    typealias Progress = @Sendable (String) -> Void

    static func launch(_ profile: ConnectionProfile, progress: Progress? = nil) async throws
        -> Outcome
    {
        guard !profile.accessKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw ConnectionError.message("请填写局域网 AccessKey")
        }
        var lastError: (any Error)?
        for baseURL in try profile.candidateBaseURLs() {
            do {
                return try await launch(profile, baseURL: baseURL, progress: progress)
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                lastError = error
            }
        }
        throw lastError ?? ConnectionError.message("没有可用的局域网地址")
    }

    private static func launch(
        _ profile: ConnectionProfile,
        baseURL: URL,
        progress: Progress?
    ) async throws -> Outcome {
        progress?("正在读取局域网连接信息…")
        let connection = try await connectionInfo(baseURL: baseURL)
        guard connection.protocol == LanConnectionInfo.currentProtocol else {
            throw ConnectionError.message("目标使用的局域网配对协议不受支持，请升级桌面端")
        }
        let api = APIClient(
            baseURL: baseURL,
            credentials: SessionCredentials(token: profile.accessKey)
        )
        let info: AppInfo = try await api.get("/api/v1/app/info")
        guard info.hasLibrary else {
            throw ConnectionError.message("桌面端没有打开的素材库")
        }
        progress?("正在等待素材库就绪…")
        try await waitUntilReady(api)
        try Task.checkCancellation()
        return Outcome(
            api: api,
            info: info,
            profile: profile.resolving(with: connection, at: baseURL)
        )
    }

    /// 初始索引期间 connection-info 暂不可用；400ms 轮询到可用。
    private static func connectionInfo(baseURL: URL) async throws -> LanConnectionInfo {
        let api = APIClient(baseURL: baseURL, credentials: SessionCredentials(token: ""))
        let deadline = Date().addingTimeInterval(30)
        while true {
            do {
                return try await api.get("/api/v1/app/lan/connection-info")
            } catch let error as APIError where error.isNotReady && Date() < deadline {
                try await Task.sleep(nanoseconds: 400_000_000)
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                throw ConnectionError.message(
                    "无法从 \(baseURL.absoluteString) 读取局域网连接信息，请确认地址与端口（默认 27375）"
                )
            }
        }
    }

    /// 初始索引构建中 daemon 只放行 app/startup；400ms 轮询到 ready。
    private static func waitUntilReady(_ api: APIClient) async throws {
        let deadline = Date().addingTimeInterval(30)
        while Date() < deadline {
            do {
                let startup: StartupInfo = try await api.get("/api/v1/app/startup")
                switch startup.status {
                case "ready":
                    return
                case "error":
                    throw ConnectionError.message(startup.message ?? "素材库索引构建失败")
                default:
                    break
                }
            } catch let error as APIError {
                if error.isUnauthorized {
                    throw error
                }
            } catch let error as ConnectionError {
                throw error
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                // 未知网络错误：继续轮询
            }
            try await Task.sleep(nanoseconds: 400_000_000)
        }
        throw ConnectionError.message("等待素材库就绪超时，请检查桌面端状态")
    }
}

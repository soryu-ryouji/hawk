import Foundation

/// 连接启动：V2 统一使用 hawk-transport 的本机 Loopback 代理；旧手动 LAN 仍可直接验证 token。
nonisolated enum ConnectionLauncher {
    struct Outcome: Sendable {
        let api: APIClient
        let info: AppInfo
        let label: String
        let targetDeviceName: String?
        let targetLibraryName: String?
    }

    typealias Progress = @Sendable (String) -> Void

    static func launch(_ profile: ConnectionProfile, progress: Progress? = nil) async throws
        -> Outcome
    {
        if !profile.targetIdentity.isEmpty {
            return try await launchNativeCandidates(profile, progress: progress)
        }
        switch profile.kind {
        case .lan:
            return try await launchLAN(profile, progress: progress)
        case .remote:
            throw ConnectionError.message("该连接缺少 target_identity，请重新配对")
        }
    }

    private static func launchNativeCandidates(
        _ profile: ConnectionProfile,
        progress: Progress?
    ) async throws -> Outcome {
        var lastError: (any Error)?
        for baseURL in try profile.candidateBaseURLs() {
            do {
                return try await launchNative(profile, baseURL: baseURL, progress: progress)
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                lastError = error
            }
        }
        throw lastError ?? ConnectionError.message("没有可用的连接地址")
    }

    private static func launchNative(
        _ profile: ConnectionProfile,
        baseURL: URL,
        progress: Progress?
    ) async throws -> Outcome {
        progress?("正在建立端到端加密会话…")
        let configDir = try transportDirectory()
        let connection = try RustTransport.connect(
            configDir: configDir,
            serverUrl: baseURL.absoluteString,
            targetDeviceId: profile.targetDeviceId,
            targetIdentity: profile.targetIdentity,
            accessKey: profile.token
        )
        guard let proxyURL = URL(string: connection.apiBase) else {
            throw ConnectionError.message("原生代理地址无效")
        }
        let api = APIClient(
            baseURL: proxyURL,
            credentials: SessionCredentials(token: connection.credential)
        )
        let info: AppInfo = try await api.get("/api/v1/app/info")
        guard info.hasLibrary else {
            throw ConnectionError.message("远端服务没有打开的素材库")
        }
        progress?("正在等待素材库就绪…")
        try await waitUntilReady(api)
        return Outcome(
            api: api,
            info: info,
            label: profile.displayName,
            targetDeviceName: nil,
            targetLibraryName: profile.targetLibraryId.isEmpty ? nil : profile.targetLibraryId
        )
    }

    private static func transportDirectory() throws -> String {
        let root = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let directory = root.appendingPathComponent("hawk/transport", isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        return directory.path
    }

    private static func launchLAN(_ profile: ConnectionProfile, progress: Progress?) async throws
        -> Outcome
    {
        guard !profile.token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw ConnectionError.message("请填写局域网访问 token")
        }
        progress?("正在验证局域网连接…")
        var lastError: (any Error)?
        for baseURL in try profile.candidateBaseURLs() {
            do {
                let api = APIClient(
                    baseURL: baseURL,
                    credentials: SessionCredentials(
                        token: profile.token.trimmingCharacters(in: .whitespacesAndNewlines)))
                let info: AppInfo = try await api.get("/api/v1/app/info")
                guard info.hasLibrary else {
                    throw ConnectionError.message("该服务没有打开的素材库，请在桌面端切换素材库后重试")
                }
                progress?("正在等待素材库就绪…")
                try await waitUntilReady(api)
                return Outcome(
                    api: api, info: info, label: profile.displayName,
                    targetDeviceName: nil, targetLibraryName: nil)
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                lastError = error
            }
        }
        throw lastError ?? ConnectionError.message("没有可用的局域网地址")
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

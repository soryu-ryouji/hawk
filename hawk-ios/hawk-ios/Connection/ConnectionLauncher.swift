import Foundation

/// V2 原生 LAN/WAN 统一经 hawk-transport；手工 LAN 的 HTTP 仅用于读取公开配对信息。
nonisolated enum ConnectionLauncher {
    struct Outcome: Sendable {
        let api: APIClient
        let info: AppInfo
        let profile: ConnectionProfile
        let targetDeviceName: String?
        let targetLibraryName: String?
        let nativeSession: NativeTransportSession?
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
            return try await launchManualLANCandidates(profile, progress: progress)
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
        let connection = try await RustTransport.connect(
            configDir: configDir,
            serverUrl: baseURL.absoluteString,
            targetDeviceId: profile.targetDeviceId,
            targetIdentity: profile.targetIdentity,
            accessKey: profile.accessKey
        )
        let nativeSession = NativeTransportSession(connection: connection)
        var transferred = false
        defer {
            if !transferred { nativeSession.close() }
        }
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
        try Task.checkCancellation()
        let outcome = Outcome(
            api: api,
            info: info,
            profile: profile,
            targetDeviceName: nil,
            targetLibraryName: profile.targetLibraryId.isEmpty ? nil : profile.targetLibraryId,
            nativeSession: nativeSession
        )
        transferred = true
        return outcome
    }

    private static func transportDirectory() throws -> String {
        let root = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let directory = root.appendingPathComponent("hawk/transport", isDirectory: true)
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        return directory.path
    }

    private static func launchManualLANCandidates(
        _ profile: ConnectionProfile,
        progress: Progress?
    ) async throws -> Outcome {
        guard !profile.accessKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw ConnectionError.message("请填写局域网 AccessKey")
        }
        var lastError: (any Error)?
        for baseURL in try profile.candidateBaseURLs() {
            let resolved: ConnectionProfile
            do {
                progress?("正在读取局域网公开配对信息…")
                let info = try await connectionInfo(baseURL: baseURL)
                guard info.protocol == LanConnectionInfo.currentProtocol else {
                    throw ConnectionError.message("目标使用的局域网配对协议不受支持，请升级桌面端")
                }
                guard !info.targetIdentity.isEmpty else {
                    throw ConnectionError.message("目标没有提供有效的 target_identity")
                }
                resolved = profile.resolving(with: info, at: baseURL)
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                lastError = ConnectionError.message(
                    "无法从 \(baseURL.absoluteString) 读取配对信息。请确认使用 hawk-app 直连端口（默认 27375），不是 LAN Web 端口"
                )
                continue
            }
            do {
                return try await launchNative(resolved, baseURL: baseURL, progress: progress)
            } catch is CancellationError {
                throw CancellationError()
            } catch {
                lastError = error
            }
        }
        throw lastError ?? ConnectionError.message("没有可用的局域网地址")
    }

    /// 初始索引期间 connection-info 暂不可用；它就绪后直接建立原生会话，无需先访问素材 API。
    private static func connectionInfo(baseURL: URL) async throws -> LanConnectionInfo {
        let api = APIClient(baseURL: baseURL, credentials: SessionCredentials(token: ""))
        let deadline = Date().addingTimeInterval(30)
        while true {
            do {
                return try await api.get("/api/v1/app/lan/connection-info")
            } catch let error as APIError where error.isNotReady && Date() < deadline {
                try await Task.sleep(nanoseconds: 400_000_000)
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

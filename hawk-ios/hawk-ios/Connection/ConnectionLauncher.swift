import Foundation

/// 广域网查看面登录：`gate` → `login` 换 bt。bt 短时效，401 时由
/// `SessionCredentials` 用它重换一次，并重新打开查看会话。
nonisolated struct RemoteAuthenticator: Sendable {
    let baseURL: URL
    let serviceKey: String
    let appKey: String

    struct SignedIn: Sendable {
        let token: String
        let deviceId: String
    }

    func signIn() async throws -> SignedIn {
        let guest = APIClient(baseURL: baseURL, credentials: SessionCredentials(token: ""))
        let gate: GateResponse = try await guest.post(
            "/api/v1/viewer/gate", GateRequest(serviceKey: serviceKey))
        let login: LoginResponse = try await guest.post(
            "/api/v1/viewer/login", LoginRequest(gateToken: gate.gateToken, key: appKey))
        return SignedIn(token: login.browserToken, deviceId: login.deviceId)
    }

    /// 重换 bt 并把查看会话重新挂到同一目标设备上
    func reopenSession(deviceId: String) async throws -> String {
        let signed = try await signIn()
        let client = APIClient(
            baseURL: baseURL, credentials: SessionCredentials(token: signed.token))
        _ =
            try await client.post(
                "/api/v1/viewer/session",
                OpenSessionRequest(targetDeviceId: deviceId, role: "viewer")
            ) as ViewerSession
        return signed.token
    }
}

/// 连接启动：局域网直接验证 token；广域网走 gate → login → session，拿到 bt 后与 daemon API 同构。
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
        switch profile.kind {
        case .lan:
            return try await launchLAN(profile, progress: progress)
        case .remote:
            return try await launchRemote(profile, progress: progress)
        }
    }

    private static func launchLAN(_ profile: ConnectionProfile, progress: Progress?) async throws
        -> Outcome
    {
        guard !profile.token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw ConnectionError.message("请填写局域网访问 token")
        }
        progress?("正在验证局域网连接…")
        var lastError: any Error?
        // 二维码一次下发所有候选地址；客户端逐个验证，哪个连通用哪个。
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

    private static func launchRemote(_ profile: ConnectionProfile, progress: Progress?) async throws
        -> Outcome
    {
        let baseURL = try profile.baseURL()
        let serviceKey = profile.serviceKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let appKey = profile.appKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !serviceKey.isEmpty else {
            throw ConnectionError.message("请填写 service-key")
        }
        guard !appKey.isEmpty else {
            throw ConnectionError.message("请填写 hawk-app key")
        }

        let authenticator = RemoteAuthenticator(
            baseURL: baseURL, serviceKey: serviceKey, appKey: appKey)
        progress?("正在登录查看面…")
        let signed = try await authenticator.signIn()

        let deviceId = signed.deviceId
        let credentials = SessionCredentials(token: signed.token) {
            try await authenticator.reopenSession(deviceId: deviceId)
        }
        let api = APIClient(baseURL: baseURL, credentials: credentials)

        progress?("正在建立查看会话…")
        // 复用或重建查看会话：服务端对存活会话直接复用，对已断开的会话重新撮合。
        let session: ViewerSession
        do {
            session = try await api.post(
                "/api/v1/viewer/session",
                OpenSessionRequest(targetDeviceId: deviceId, role: "viewer")
            )
        } catch let error as APIError {
            throw ConnectionError.message(error.message)
        }

        let info: AppInfo = try await api.get("/api/v1/app/info")
        guard info.hasLibrary else {
            throw ConnectionError.message("远端服务没有打开的素材库")
        }
        progress?("正在等待素材库就绪…")
        try await waitUntilReady(api)
        let label = session.targetName.isEmpty ? profile.displayName : session.targetName
        return Outcome(
            api: api, info: info, label: label,
            targetDeviceName: session.targetName, targetLibraryName: session.targetLibraryName)
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
                // NOT_READY、网络抖动与服务端 5xx 都继续轮询，直到超时
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

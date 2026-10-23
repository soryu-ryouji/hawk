import Foundation

/// 连接启动：局域网直接验证 token；广域网走 gate → login → session，拿到 bt 后与 daemon API 同构。
nonisolated enum ConnectionLauncher {
    struct Outcome: Sendable {
        let api: APIClient
        let info: AppInfo
        let label: String
        let targetDeviceName: String?
        let targetLibraryName: String?
    }

    static func launch(_ profile: ConnectionProfile) async throws -> Outcome {
        let baseURL = try profile.baseURL()
        switch profile.kind {
        case .lan:
            guard !profile.token.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                throw ConnectionError.message("请填写局域网访问 token")
            }
            let api = APIClient(
                baseURL: baseURL,
                token: profile.token.trimmingCharacters(in: .whitespacesAndNewlines))
            let info: AppInfo = try await api.get("/api/v1/app/info")
            guard info.hasLibrary else {
                throw ConnectionError.message("该服务没有打开的素材库，请在桌面端切换素材库后重试")
            }
            try await waitUntilReady(api)
            return Outcome(
                api: api,
                info: info,
                label: profile.displayName,
                targetDeviceName: nil,
                targetLibraryName: nil
            )

        case .remote:
            let serviceKey = profile.serviceKey.trimmingCharacters(in: .whitespacesAndNewlines)
            let appKey = profile.appKey.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !serviceKey.isEmpty else {
                throw ConnectionError.message("请填写 service-key")
            }
            guard !appKey.isEmpty else {
                throw ConnectionError.message("请填写 hawk-app key")
            }

            let guest = APIClient(baseURL: baseURL, token: "")
            let gate: GateResponse = try await guest.post(
                "/api/v1/viewer/gate", GateRequest(serviceKey: serviceKey))
            let login: LoginResponse = try await guest.post(
                "/api/v1/viewer/login",
                LoginRequest(gateToken: gate.gateToken, key: appKey)
            )
            let api = APIClient(baseURL: baseURL, token: login.token)

            // 复用或重建查看会话：服务端对存活会话直接复用，对已断开的会话重新撮合。
            // 不用 GET /viewer/session 走捷径——旧会话可能已死，跳过 POST 会卡在失效隧道上。
            let session: ViewerSession
            do {
                session = try await api.post(
                    "/api/v1/viewer/session",
                    OpenSessionRequest(targetDeviceId: login.deviceId, access: "read")
                )
            } catch let error as APIError {
                throw ConnectionError.message(error.message)
            }

            let info: AppInfo = try await api.get("/api/v1/app/info")
            guard info.hasLibrary else {
                throw ConnectionError.message("远端服务没有打开的素材库")
            }
            try await waitUntilReady(api)
            let label = session.targetName.isEmpty ? profile.displayName : session.targetName
            return Outcome(
                api: api,
                info: info,
                label: label,
                targetDeviceName: session.targetName,
                targetLibraryName: session.targetLibraryName
            )
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

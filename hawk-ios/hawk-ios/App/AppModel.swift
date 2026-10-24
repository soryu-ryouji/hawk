import Foundation
import Observation

@MainActor
@Observable
final class AppModel {
    enum Phase: Equatable {
        case launching
        case connect
        case connecting
        case ready
    }

    private(set) var phase: Phase = .launching
    private(set) var context: LibraryContext?
    private(set) var lastError: String?
    private(set) var connectStatus: String?
    let connections = ConnectionStore()

    private var connectTask: Task<Void, Never>?
    private var started = false

    func start() {
        guard !started else { return }
        started = true
        #if DEBUG
            seedDebugProfileIfNeeded()
        #endif
        if let profile = connections.activeProfile {
            connect(profile)
        } else {
            phase = .connect
        }
    }

    func connect(_ profile: ConnectionProfile) {
        connectTask?.cancel()
        context?.stopEvents()
        teardownRemoteSession()
        context = nil
        lastError = nil
        connectStatus = "正在连接 \(profile.displayName)…"
        phase = .connecting

        connectTask = Task { [weak self] in
            guard let self else { return }
            do {
                let outcome = try await ConnectionLauncher.launch(profile) { [weak self] status in
                    Task { @MainActor in
                        self?.connectStatus = status
                    }
                }
                try Task.checkCancellation()
                let saved = connections.upsert(profile)
                connections.setActive(saved.id)

                self.connectStatus = "正在读取素材库…"
                let context = LibraryContext(
                    api: outcome.api,
                    profile: saved,
                    info: outcome.info,
                    targetDeviceName: outcome.targetDeviceName,
                    targetLibraryName: outcome.targetLibraryName
                )
                try await context.bootstrap()
                try Task.checkCancellation()
                self.context = context
                self.connectStatus = nil
                phase = .ready
                // 分享扩展读 App Group：局域网连接成功即写；远端连接保留上一次的局域网目标
                ShareTarget.save(profile: saved, libraryName: context.library?.name ?? "")
            } catch is CancellationError {
                return
            } catch {
                guard !Task.isCancelled else { return }
                lastError = errorMessage(error)
                connectStatus = nil
                phase = .connect
            }
        }
    }

    func saveAndConnect(_ profile: ConnectionProfile) {
        let saved = connections.upsert(profile)
        connect(saved)
    }

    func reconnect() {
        if let profile = context?.profile ?? connections.activeProfile {
            connect(profile)
        }
    }

    func disconnect() {
        connectTask?.cancel()
        context?.stopEvents()
        teardownRemoteSession()
        context = nil
        connectStatus = nil
        phase = .connect
        ShareTarget.clear()
    }

    func showConnectScreen() {
        connectTask?.cancel()
        context?.stopEvents()
        teardownRemoteSession()
        context = nil
        connectStatus = nil
        phase = .connect
        ShareTarget.clear()
    }

    /// 显式断开/切换时通知服务端注销查看会话（best-effort；失败不阻塞 UI）。
    private func teardownRemoteSession() {
        guard let context, context.isRemote else { return }
        let api = context.api
        Task {
            try? await api.delete("/api/v1/viewer/session")
        }
    }

    func deleteProfile(_ profile: ConnectionProfile) {
        connections.remove(profile.id)
        if context?.profile.id == profile.id {
            disconnect()
        }
    }

    func setBackgrounded(_ backgrounded: Bool) {
        context?.setBackgrounded(backgrounded)
    }

    #if DEBUG
        /// 本地冒烟：模拟器/真机自动化测试可通过环境变量预置一条连接，免去手动输入。
        /// Keychain 里的旧连接不会随卸载清除，因此每次都覆盖同 id 记录并设为当前连接。
        private func seedDebugProfileIfNeeded() {
            guard
                let raw = ProcessInfo.processInfo.environment["HAWK_IOS_DEBUG_PROFILE"],
                let data = raw.data(using: .utf8),
                let profile = try? JSONDecoder().decode(ConnectionProfile.self, from: data)
            else {
                return
            }
            let saved = connections.upsert(profile)
            connections.setActive(saved.id)
        }
    #endif

    private func errorMessage(_ error: any Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}

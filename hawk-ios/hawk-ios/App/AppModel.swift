import Foundation
import Observation

@MainActor
@Observable
final class AppModel {
    enum Phase: Equatable {
        case launching
        case connect
        case connecting(String)
        case ready
    }

    private(set) var phase: Phase = .launching
    private(set) var session: LibrarySession?
    private(set) var lastError: String?
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
        teardownRemoteSession()
        session = nil
        lastError = nil
        phase = .connecting("正在连接 \(profile.displayName)…")

        connectTask = Task { [weak self] in
            guard let self else { return }
            do {
                let outcome = try await ConnectionLauncher.launch(profile)
                try Task.checkCancellation()
                let saved = connections.upsert(profile)
                connections.setActive(saved.id)

                let store = LibraryStore(api: outcome.api)
                try await store.bootstrap()
                try Task.checkCancellation()
                let label: String
                if let targetLibrary = outcome.targetLibraryName, !targetLibrary.isEmpty {
                    label = "\(outcome.label) · \(targetLibrary)"
                } else {
                    label = store.library?.name ?? outcome.label
                }
                session = LibrarySession(
                    api: outcome.api,
                    profile: saved,
                    label: label,
                    access: outcome.info.access,
                    targetDeviceName: outcome.targetDeviceName,
                    targetLibraryName: outcome.targetLibraryName,
                    store: store
                )
                phase = .ready
            } catch is CancellationError {
                return
            } catch {
                guard !Task.isCancelled else { return }
                lastError = errorMessage(error)
                phase = .connect
            }
        }
    }

    func saveAndConnect(_ profile: ConnectionProfile) {
        let saved = connections.upsert(profile)
        connect(saved)
    }

    func reconnect() {
        if let profile = session?.profile ?? connections.activeProfile {
            connect(profile)
        }
    }

    func disconnect() {
        connectTask?.cancel()
        teardownRemoteSession()
        session = nil
        phase = .connect
    }

    func showConnectScreen() {
        connectTask?.cancel()
        teardownRemoteSession()
        session = nil
        phase = .connect
    }

    /// 显式断开/切换时通知服务端注销查看会话（best-effort；失败不阻塞 UI）。
    private func teardownRemoteSession() {
        guard let session, session.isRemote else { return }
        let api = session.api
        Task {
            try? await api.delete("/api/v1/viewer/session")
        }
    }

    func deleteProfile(_ profile: ConnectionProfile) {
        connections.remove(profile.id)
        if session?.profile.id == profile.id {
            disconnect()
        }
    }

    #if DEBUG
        /// 本地冒烟：模拟器/真机自动化测试可通过环境变量预置一条连接，免去手动输入。
        private func seedDebugProfileIfNeeded() {
            guard connections.profiles.isEmpty,
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

    private func errorMessage(_ error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}

import Foundation
import Observation

@MainActor
@Observable
final class ConnectionStore {
    private(set) var profiles: [ConnectionProfile] = []
    private(set) var activeID: UUID?

    init() {
        if let bookmark = KeychainStore.loadBookmark() {
            profiles = bookmark.profiles
            activeID = bookmark.activeID
        }
    }

    var activeProfile: ConnectionProfile? {
        guard let activeID else { return nil }
        return profiles.first { $0.id == activeID }
    }

    /// 同一类型 + 同一地址视为同一条连接（更新凭证而不是堆叠重复项）。
    @discardableResult
    func upsert(_ profile: ConnectionProfile) -> ConnectionProfile {
        // 编辑已有连接：按 id 原位更新（即使地址被改掉，不能退化成新增 + 残留旧项）。
        if let index = profiles.firstIndex(where: { $0.id == profile.id }) {
            profiles[index] = profile
            persist()
            return profile
        }
        // 原生连接按目标身份去重：同一 hawk-server 下可以有多个设备/素材库。
        let normalized = profile.url.trimmingCharacters(in: .whitespacesAndNewlines)
        if let index = profiles.firstIndex(where: { existing in
            guard existing.kind == profile.kind else { return false }
            if !profile.targetIdentity.isEmpty {
                return existing.targetIdentity == profile.targetIdentity
                    && existing.targetLibraryId == profile.targetLibraryId
            }
            return existing.targetIdentity.isEmpty
                && existing.url.caseInsensitiveCompare(normalized) == .orderedSame
        }) {
            var updated = profile
            updated.id = profiles[index].id
            profiles[index] = updated
            persist()
            return updated
        }
        profiles.append(profile)
        persist()
        return profile
    }

    func setActive(_ id: UUID?) {
        activeID = id
        persist()
    }

    func remove(_ id: UUID) {
        profiles.removeAll { $0.id == id }
        if activeID == id {
            activeID = nil
        }
        persist()
    }

    private func persist() {
        KeychainStore.saveBookmark(ConnectionBookmark(profiles: profiles, activeID: activeID))
    }
}

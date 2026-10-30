import Foundation
import Observation

/// 一个素材库会话的共享只读上下文：库信息、标签体系、计数、锁、隐藏集与排序偏好。
/// 网格页面（`GridStore`）各自持有视图与查询状态，共享这里的数据，切页不重复请求。
@MainActor
@Observable
final class LibraryContext {
    enum LiveState: Equatable {
        case connecting
        case live
        case reconnecting
        case paused

        var isDegraded: Bool { self == .reconnecting }
    }

    let api: APIClient
    let profile: ConnectionProfile
    let info: AppInfo
    let targetDeviceName: String?
    let targetLibraryName: String?
    let tickets: UnlockTicketStore

    private(set) var library: LibraryInfo?
    private(set) var folders: FolderNode?
    private(set) var categories: [TaxonInfo] = []
    private(set) var tags: [TaxonInfo] = []
    private(set) var counts = LibraryCounts()
    private(set) var globalFilter: GlobalFilterSnapshot = .empty
    private(set) var locks: LockSnapshot = .empty
    private(set) var sortPreferences: [String: ViewSortPreference] = [:]
    private(set) var taxonomyLoading = true
    private(set) var countsLoading = true
    private(set) var scanning: TaskProgress?
    private(set) var liveState: LiveState = .paused
    /// 成员关系可能已变化（增删/移动/锁/隐藏集）：网格据此重查骨架
    private(set) var structureRevision = 0
    /// SSE 增量更新的条目详情（按 key），网格优先使用
    private(set) var updatedItems: [String: Item] = [:]
    /// 最近一次刷新时间，回前台节流用
    private(set) var lastRefreshedAt: Date?

    var toast: String?

    private var deferredTask: Task<Void, Never>?
    private var taxonomyTask: Task<Void, Never>?
    private var eventsTask: Task<Void, Never>?
    private var backgrounded = false

    init(
        api: APIClient, profile: ConnectionProfile, info: AppInfo,
        targetDeviceName: String?, targetLibraryName: String?
    ) {
        self.api = api
        self.profile = profile
        self.info = info
        self.targetDeviceName = targetDeviceName
        self.targetLibraryName = targetLibraryName
        self.tickets = api.tickets
    }

    var isRemote: Bool { profile.kind == .remote }
    var canWrite: Bool { info.role != "viewer" }

    var accessLabel: String {
        switch info.role {
        case "admin": return "完全访问"
        case "editor": return "可读写"
        case "importer": return "仅导入"
        default: return "只读"
        }
    }

    var title: String {
        let libraryName = library?.name ?? targetLibraryName ?? "素材库"
        if let target = targetDeviceName, !target.isEmpty, isRemote {
            return "\(target) · \(libraryName)"
        }
        return libraryName
    }

    var scanningLabel: String? {
        guard let scanning, scanning.isBusy else { return nil }
        return scanning.label
    }

    // MARK: - Bootstrap

    /// 首屏所需的最小集合：库信息、隐藏集、锁。分类/标签、计数、排序偏好在后台补齐。
    func bootstrap() async throws {
        async let libraryTask: LibraryInfo = api.get("/api/v1/library/info")
        async let filterTask: GlobalFilterSnapshot = api.get("/api/v1/global_filter/list")
        async let locksTask: LockSnapshot = api.get("/api/v1/lock/list")
        async let prefsTask: [String: ViewSortPreference] = api.get("/api/v1/view/preferences")

        library = try await libraryTask
        globalFilter = (try? await filterTask) ?? .empty
        locks = (try? await locksTask) ?? .empty
        sortPreferences = (try? await prefsTask) ?? [:]
        lastRefreshedAt = Date()

        deferredTask?.cancel()
        deferredTask = Task { [weak self] in
            guard let self else { return }
            async let taxonomy: Void = self.refreshTaxonomy()
            async let counters: Void = self.refreshCounts()
            _ = await (taxonomy, counters)
            await self.loadTaskStatus()
            self.startEvents()
        }
    }

    func refreshIfStale(minInterval: TimeInterval = 90) async {
        guard Date().timeIntervalSince(lastRefreshedAt ?? .distantPast) > minInterval else {
            return
        }
        await refreshAll()
    }

    func refreshAll() async {
        async let taxonomy: Void = refreshTaxonomy()
        async let counters: Void = refreshCounts()
        _ = await (taxonomy, counters)
        structureRevision += 1
        lastRefreshedAt = Date()
        restartEventsIfNeeded()
    }

    func refreshTaxonomy() async {
        taxonomyLoading = true
        defer { taxonomyLoading = false }
        do {
            async let foldersTask: FolderNode = api.get("/api/v1/folder/list")
            async let categoriesTask: [TaxonInfo] = api.get("/api/v1/category/list")
            async let tagsTask: [TaxonInfo] = api.get("/api/v1/tag/list")
            let (folders, categories, tags) = try await (foldersTask, categoriesTask, tagsTask)
            self.folders = folders
            self.categories = categories
            self.tags = tags
        } catch {
            // 维度刷新失败不打断主列表
        }
    }

    func refreshCounts() async {
        countsLoading = true
        defer { countsLoading = false }
        async let all = count(view: .all)
        async let root = count(view: .root)
        async let uncategorized = count(view: .uncategorized)
        async let untagged = count(view: .untagged)
        async let trash = count(view: .trash)
        let values = await (all, root, uncategorized, untagged, trash)
        counts = LibraryCounts(
            all: values.0, root: values.1, uncategorized: values.2, untagged: values.3,
            trash: values.4)
    }

    private func count(view: LibraryView) async -> Int {
        do {
            var request = request(for: view, query: QueryState())
            request.limit = 1
            let response: ItemListResponse = try await api.post("/api/v1/item/list", request)
            return response.total
        } catch {
            return 0
        }
    }

    /// 背景索引/缩略图积压：SSE 未连上时也能给出进度提示
    private func loadTaskStatus() async {
        guard let status: TaskStatus = try? await api.get("/api/v1/app/status") else { return }
        let backlog = status.index
        if backlog.isBusy {
            scanning = TaskProgress(
                task: "index", pending: backlog.pending, active: backlog.active,
                phase: backlog.phase, processed: backlog.processed, total: backlog.total)
        }
    }

    // MARK: - 锁

    func lockTarget(for view: LibraryView) -> LockTarget? {
        LockCoverage.target(for: view, in: locks)
    }

    func isLocked(_ view: LibraryView) -> Bool {
        lockTarget(for: view) != nil
    }

    func isUnlocked(_ view: LibraryView) -> Bool {
        guard let target = lockTarget(for: view) else { return false }
        return tickets.contains(target)
    }

    func folderIsLocked(_ path: String) -> Bool {
        LockCoverage.folderIsLocked(path, in: locks)
    }

    func isUnlocked(folder path: String) -> Bool {
        guard let target = LockCoverage.target(for: .folder(path), in: locks) else { return false }
        return tickets.contains(target)
    }

    func isUnlocked(category name: String) -> Bool {
        tickets.contains(LockTarget(dimension: .category, name: name))
    }

    func isUnlocked(tag name: String) -> Bool {
        tickets.contains(LockTarget(dimension: .tag, name: name))
    }

    func categoryIsLocked(_ name: String) -> Bool {
        locks.categories.contains(name)
    }

    func tagIsLocked(_ name: String) -> Bool {
        locks.tags.contains(name)
    }

    func unlock(_ target: LockTarget, password: String) async throws {
        let response: UnlockResponse = try await api.post(
            "/api/v1/lock/unlock",
            UnlockRequest(
                dimension: target.dimension.rawValue, name: target.name, password: password)
        )
        tickets.add(response.unlockToken, for: target)
        structureRevision += 1
        scheduleTaxonomyRefresh()
    }

    func relock(_ target: LockTarget) {
        tickets.drop(target)
        structureRevision += 1
        scheduleTaxonomyRefresh()
    }

    private func scheduleTaxonomyRefresh() {
        taxonomyTask?.cancel()
        taxonomyTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 500_000_000)
            guard !Task.isCancelled else { return }
            self?.structureRevision += 1
            await self?.refreshTaxonomy()
            await self?.refreshCounts()
        }
    }

    // MARK: - 排序偏好（服务端 scope 记忆，与桌面端同步）

    /// 每个视图的滚动位置（行首 item key），切走再回来时恢复
    var scrollAnchors: [String: String] = [:]

    func sortPreference(for view: LibraryView) -> ViewSortPreference? {
        guard let scope = view.sortScope else { return nil }
        if let direct = sortPreferences[scope] { return direct }
        // 文件夹沿父链继承（服务端只存扁平 scope，解析由客户端做）
        if case .folder(let path) = view {
            var current = path
            while let slash = current.lastIndex(of: "/") {
                current = String(current[..<slash])
                if let inherited = sortPreferences["folder:\(current)"] { return inherited }
            }
            return sortPreferences["folder:"]
        }
        return nil
    }

    func saveSortPreference(_ sort: ViewSortPreference, for view: LibraryView) {
        guard let scope = view.sortScope else { return }
        sortPreferences[scope] = sort
        guard canWrite else { return }
        Task { [api] in
            try? await api.put(
                "/api/v1/view/preference",
                ViewPreferenceRequest(scope: scope, orderBy: sort.orderBy, order: sort.order))
        }
    }

    // MARK: - 请求构建

    func request(for view: LibraryView, query: QueryState) -> ItemListRequest {
        var request = ItemListRequest(orderBy: query.orderBy, order: query.order)
        if !query.keywords.isEmpty {
            request.keywords = query.keywords
        }
        request.star = query.star
        request.color = query.color
        if case .side(let min, let max) = query.size {
            request.minSideGte = min
            request.minSideLte = max
        }

        switch view {
        case .all:
            break
        case .root:
            request.folders = [""]
            request.foldersExact = true
        case .uncategorized:
            request.withoutCategories = true
        case .untagged:
            request.withoutTags = true
        case .trash:
            request.inTrash = true
        case .folder(let path):
            request.folders = [path]
        case .category(let name):
            request.categories = [name]
        case .tag(let name):
            request.tags = [name]
        }

        if view.isGlobal {
            if !globalFilter.folders.isEmpty { request.excludeFolders = globalFilter.folders }
            if !globalFilter.categories.isEmpty {
                request.excludeCategories = globalFilter.categories
            }
            if !globalFilter.tags.isEmpty { request.excludeTags = globalFilter.tags }
        }
        return request
    }

    // MARK: - SSE

    func startEvents() {
        guard eventsTask == nil else { return }
        liveState = .connecting
        eventsTask = Task { [weak self] in
            var backoff: UInt64 = 1
            while !Task.isCancelled {
                guard let self else { return }
                do {
                    for try await event in EventStream.events(api: self.api) {
                        self.liveState = .live
                        backoff = 1
                        self.handle(event)
                    }
                } catch {
                    // 断开：按退避重连，重连成功后全量对齐
                }
                guard !Task.isCancelled else { break }
                self.liveState = .reconnecting
                try? await Task.sleep(nanoseconds: backoff * 1_000_000_000)
                backoff = min(backoff * 2, 15)
                guard !Task.isCancelled else { break }
                await self.realignAfterReconnect()
            }
        }
    }

    func stopEvents() {
        eventsTask?.cancel()
        eventsTask = nil
        liveState = .paused
    }

    /// App 进出前后台：前台恢复订阅，后台断开（系统会挂起连接，留着只会假在线）。
    func setBackgrounded(_ value: Bool) {
        backgrounded = value
        if value {
            stopEvents()
        } else {
            startEvents()
        }
    }

    private func restartEventsIfNeeded() {
        guard !backgrounded, eventsTask == nil else { return }
        startEvents()
    }

    private func realignAfterReconnect() async {
        async let taxonomy: Void = refreshTaxonomy()
        async let counters: Void = refreshCounts()
        async let prefs: Void = reloadSortPreferences()
        _ = await (taxonomy, counters, prefs)
        structureRevision += 1
        lastRefreshedAt = Date()
    }

    private func reloadSortPreferences() async {
        if let prefs: [String: ViewSortPreference] = try? await api.get(
            "/api/v1/view/preferences")
        {
            sortPreferences = prefs
        }
    }

    private func handle(_ event: ServerEvent) {
        switch event.name {
        case "item.updated", "items.updated":
            applyItemUpdates(event.data)
        case "item.added", "items.added", "item.removed", "item.trashed", "item.restored":
            structureRevision += 1
        case "folder.changed":
            structureRevision += 1
            scheduleTaxonomyRefresh()
        case "locks.changed":
            if let snapshot = decode(LockSnapshot.self, from: event.data) {
                locks = snapshot
                structureRevision += 1
                scheduleTaxonomyRefresh()
            }
        case "global_filter.changed":
            if let snapshot = decode(GlobalFilterSnapshot.self, from: event.data) {
                globalFilter = snapshot
                structureRevision += 1
            }
        case "library.updated":
            if let info = decode(LibraryInfo.self, from: event.data) {
                library = info
            }
        case "task.progress":
            if let progress = decode(TaskProgress.self, from: event.data) {
                scanning = progress.isBusy ? progress : nil
            }
        default:
            break
        }
    }

    private func applyItemUpdates(_ data: Data) {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        var updated: [Item] = []
        if let single = try? decoder.decode(Item.self, from: data) {
            updated = [single]
        } else if let batch = try? decoder.decode(ItemsPayload.self, from: data) {
            updated = batch.items
        }
        guard !updated.isEmpty else {
            structureRevision += 1
            return
        }
        if updatedItems.count > 2000 { updatedItems.removeAll(keepingCapacity: true) }
        for item in updated {
            updatedItems[item.key] = item
        }
    }

    private func decode<T: Decodable>(_ type: T.Type, from data: Data) -> T? {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try? decoder.decode(type, from: data)
    }
}

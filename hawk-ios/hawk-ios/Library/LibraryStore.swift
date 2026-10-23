import Foundation
import Observation

/// 一个已连接的素材库会话：API 客户端 + 浏览状态。
@MainActor
@Observable
final class LibrarySession: Identifiable {
    let id = UUID()
    let api: APIClient
    let profile: ConnectionProfile
    let label: String
    let access: String
    let targetDeviceName: String?
    let targetLibraryName: String?
    let store: LibraryStore

    init(
        api: APIClient, profile: ConnectionProfile, label: String, access: String,
        targetDeviceName: String?, targetLibraryName: String?, store: LibraryStore
    ) {
        self.api = api
        self.profile = profile
        self.label = label
        self.access = access
        self.targetDeviceName = targetDeviceName
        self.targetLibraryName = targetLibraryName
        self.store = store
    }

    var isRemote: Bool { profile.kind == .remote }
    var accessLabel: String {
        switch access {
        case "admin": "完全访问"
        case "remote_writer": "受限写入"
        default: "只读"
        }
    }
}

@MainActor
@Observable
final class LibraryStore {
    let api: APIClient

    private(set) var library: LibraryInfo?
    private(set) var folders: FolderNode?
    private(set) var categories: [TaxonInfo] = []
    private(set) var tags: [TaxonInfo] = []
    private(set) var globalFilter: GlobalFilterSnapshot = .empty
    private(set) var counts = LibraryCounts()

    var view: LibraryView = .all
    var query = QueryState()
    var searchText: String = ""

    private(set) var skeleton: [SkeletonItem] = []
    private(set) var details: [String: Item] = [:]
    private(set) var totalSize: Int64 = 0
    private(set) var loading = false
    private(set) var error: String?
    private(set) var skeletonVersion = 0
    /// 网格布局版本：首屏分页内容、完整骨架、清空都会推进，驱动 ItemGridView 重排
    private(set) var layoutVersion = 0
    /// 首屏分页内容（item/list 首页）：完整骨架到达前先渲染，避免等待全量骨架
    private(set) var provisionalSkeleton: [SkeletonItem] = []
    /// 分类/标签与智能条目计数是否仍在加载（侧栏据此显示占位）
    private(set) var taxonomyLoading = true
    private(set) var countsLoading = true
    private(set) var firstPageLoading = false
    private var lastLoadedAt: Date?

    private var reloadTask: Task<Void, Never>?
    private var searchTask: Task<Void, Never>?
    private var contentTask: Task<Void, Never>?
    private var loadedPages: Set<Int> = []
    private var pendingPages: Set<Int> = []
    private let pageSize = 48

    init(api: APIClient) {
        self.api = api
    }

    var hasActiveFilters: Bool { query.hasActiveFilters }
    /// 当前实际展示的骨架：优先完整骨架；骨架未到时用首屏分页内容占位
    var displayItems: [SkeletonItem] { skeleton.isEmpty ? provisionalSkeleton : skeleton }
    var total: Int { displayItems.count }
    var currentSort: SortChoice? { SortChoice.current(query) }

    var viewTitle: String { view.title }

    // MARK: - Bootstrap

    /// 只做“能出界面”的最小准备：库信息 + 全局隐藏集。
    /// 分类/标签、首屏素材、完整骨架、计数全部在后台补齐——主界面不再等待全量数据。
    func bootstrap() async throws {
        async let libraryTask: LibraryInfo = api.get("/api/v1/library/info")
        let filter: GlobalFilterSnapshot =
            (try? await api.get("/api/v1/global_filter/list")) ?? .empty
        let library = try await libraryTask
        self.library = library
        self.globalFilter = filter
        startDeferredLoad()
    }

    /// 后台补齐：首屏分页与完整骨架并发；随后加载分类/标签与计数。
    private func startDeferredLoad() {
        contentTask?.cancel()
        loading = true
        contentTask = Task { [weak self] in
            guard let self else { return }
            async let content: Void = self.loadContentWithPreview()
            async let taxonomy: Void = self.refreshTaxonomy()
            _ = await (content, taxonomy)
            // 计数是 5 个列表查询，放在内容之后，避免和首屏骨架抢 daemon
            await self.refreshCounts()
        }
    }

    /// 首屏分页与完整骨架并发：分页先到就先渲染，骨架到达后接管完整虚拟网格。
    private func loadContentWithPreview() async {
        async let preview: Void = loadFirstPagePreview()
        async let fullSkeleton: Void = reload()
        _ = await (preview, fullSkeleton)
    }

    /// 回到前台时刷新（节流）：浏览期间素材可能在桌面端被增删改。
    func refreshIfStale(minInterval: TimeInterval = 120) async {
        guard Date().timeIntervalSince(lastLoadedAt ?? .distantPast) > minInterval else {
            return
        }
        await refreshAll()
    }

    func refreshAll() async {
        async let taxonomy: Void = refreshTaxonomy()
        async let counters: Void = refreshCounts()
        async let content: Void = reload()
        _ = await (taxonomy, counters, content)
    }

    private func refreshTaxonomy() async {
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
            globalFilter = (try? await api.get("/api/v1/global_filter/list")) ?? .empty
        } catch {
            // 分类维度刷新失败不打断主列表
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
            all: values.0,
            root: values.1,
            uncategorized: values.2,
            untagged: values.3,
            trash: values.4
        )
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

    // MARK: - View / query

    func select(view: LibraryView) {
        guard self.view != view else { return }
        self.view = view
        scheduleReload(debounceMilliseconds: 0)
    }

    /// 返回上层：文件夹回到父目录（根目录回全部素材），其他维度回到全部素材。
    func navigateUp() {
        switch view {
        case .folder(let path):
            if let slash = path.lastIndex(of: "/") {
                select(view: .folder(String(path[..<slash])))
            } else {
                select(view: .root)
            }
        default:
            select(view: .all)
        }
    }

    func setSearchText(_ text: String) {
        searchText = text
        searchTask?.cancel()
        searchTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 420_000_000)
            guard !Task.isCancelled, let self else { return }
            self.applySearch()
        }
    }

    func submitSearch() {
        searchTask?.cancel()
        applySearch()
    }

    private func applySearch() {
        let keywords =
            searchText
            .split(whereSeparator: { $0.isWhitespace })
            .map(String.init)
        guard keywords != query.keywords else { return }
        query.keywords = keywords
        scheduleReload(debounceMilliseconds: 0)
    }

    func setSort(_ choice: SortChoice) {
        guard query.orderBy != choice.orderBy || query.order != choice.order else { return }
        query.orderBy = choice.orderBy
        query.order = choice.order
        scheduleReload(debounceMilliseconds: 0)
    }

    func setStar(_ star: Int?) {
        query.star = star
        scheduleReload(debounceMilliseconds: 0)
    }

    func setSize(_ size: SizeFilter?) {
        query.size = size
        scheduleReload(debounceMilliseconds: 0)
    }

    func setColor(_ color: String?) {
        query.color = color
        scheduleReload(debounceMilliseconds: 0)
    }

    func clearFilters() {
        query.star = nil
        query.color = nil
        query.size = nil
        scheduleReload(debounceMilliseconds: 0)
    }

    /// 筛选面板一次性应用全部条件（避免每个维度各触发一轮骨架请求）。
    func applyFilters(star: Int?, color: String?, size: SizeFilter?) {
        query.star = star
        query.color = color
        query.size = size
        scheduleReload(debounceMilliseconds: 0)
    }

    // MARK: - Skeleton

    /// 首屏分页：完整骨架到达前先显示第一页素材（limit 48），让内容尽早出现。
    private func loadFirstPagePreview() async {
        guard skeleton.isEmpty, provisionalSkeleton.isEmpty else { return }
        let viewSnapshot = view
        let querySnapshot = query
        firstPageLoading = true
        defer { firstPageLoading = false }

        var request = request(for: viewSnapshot, query: querySnapshot)
        request.offset = 0
        request.limit = pageSize
        do {
            let response: ItemListResponse = try await api.post("/api/v1/item/list", request)
            guard !Task.isCancelled,
                skeleton.isEmpty,
                view == viewSnapshot,
                query == querySnapshot
            else {
                return
            }
            guard !response.items.isEmpty else { return }

            for item in response.items {
                details[item.key] = item
            }
            provisionalSkeleton = response.items.map {
                SkeletonItem(
                    id: $0.id, path: $0.path, width: $0.width, height: $0.height,
                    star: $0.star, size: $0.size)
            }
            totalSize = response.totalSize
            layoutVersion += 1
        } catch {
            // 首屏失败静默：完整骨架/下拉刷新会接管
        }
    }

    func reload() async {
        do {
            try await loadSkeletonNow()
        } catch {
            if error is CancellationError { return }
            self.error = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    private func scheduleReload(debounceMilliseconds: UInt64) {
        // 换视图/筛选：清掉上一个查询的内容，先显示占位与首屏分页，不等完整骨架
        clearQueryContent()
        reloadTask?.cancel()
        reloadTask = Task { [weak self] in
            if debounceMilliseconds > 0 {
                try? await Task.sleep(nanoseconds: debounceMilliseconds * 1_000_000)
            }
            guard !Task.isCancelled, let self else { return }
            await self.loadContentWithPreview()
        }
    }

    /// 查询条件切换时的内容重置：旧 skeleton/详情/分页状态全部作废，避免新视图下显示旧内容。
    private func clearQueryContent() {
        skeleton = []
        provisionalSkeleton = []
        details.removeAll(keepingCapacity: true)
        totalSize = 0
        skeletonVersion += 1
        layoutVersion += 1
        loadedPages.removeAll()
        pendingPages.removeAll()
        error = nil
        loading = true
    }

    private func loadSkeletonNow() async throws {
        skeletonVersion += 1
        let version = skeletonVersion
        loading = true
        error = nil
        defer {
            if version == skeletonVersion {
                loading = false
            }
        }

        let request = request(for: view, query: query)
        let response: ItemSkeletonResponse = try await api.post("/api/v1/item/skeleton", request)
        guard version == skeletonVersion else { return }

        skeleton = response.items
        totalSize = response.totalSize
        lastLoadedAt = Date()
        provisionalSkeleton.removeAll()
        layoutVersion += 1
        // 保留首屏分页已取到的详情；只清掉不属于当前视图的条目
        let validKeys = Set(response.items.map(\.key))
        for key in Array(details.keys) where !validKeys.contains(key) {
            details.removeValue(forKey: key)
        }
        loadedPages.removeAll()
        pendingPages.removeAll()
        // 首屏分页已经逐条覆盖第一页时无需再拉一遍 item/list
        if !response.items.isEmpty,
            response.items.prefix(pageSize).allSatisfy({ details[$0.key] != nil })
        {
            loadedPages.insert(0)
        }
        ensureWindow(0..<min(pageSize, response.items.count))
    }

    // MARK: - Details

    func ensureWindow(_ range: Range<Int>) {
        guard !skeleton.isEmpty, !range.isEmpty else { return }
        let lower = max(0, range.lowerBound - 12)
        let upper = min(skeleton.count, range.upperBound + 12)
        guard lower < upper else { return }
        let firstPage = lower / pageSize
        let lastPage = (upper - 1) / pageSize
        for page in firstPage...lastPage {
            loadPage(page)
        }
    }

    private func loadPage(_ page: Int) {
        guard page >= 0 else { return }
        let key = page
        if loadedPages.contains(key) || pendingPages.contains(key) { return }
        let version = skeletonVersion
        pendingPages.insert(key)

        Task { [weak self] in
            guard let self else { return }
            defer {
                if self.skeletonVersion == version {
                    self.pendingPages.remove(key)
                }
            }
            var request = self.request(for: self.view, query: self.query)
            request.offset = page * self.pageSize
            request.limit = self.pageSize
            do {
                let response: ItemListResponse = try await self.api.post(
                    "/api/v1/item/list", request)
                guard self.skeletonVersion == version else { return }
                for item in response.items {
                    self.details[item.key] = item
                }
                self.loadedPages.insert(key)
            } catch {
                // 视口窗口失败静默，后续 onAppear/刷新会重试
            }
        }
    }

    /// 预览需要单条完整元数据（标签/分类/备注）：优先用窗口缓存，否则按 id + path 拉一次。
    func detail(at index: Int) async -> Item? {
        let items = displayItems
        guard items.indices.contains(index) else { return nil }
        let item = items[index]
        if let cached = details[item.key] {
            return cached
        }
        do {
            let detail: Item = try await api.get(
                "/api/v1/item/detail",
                query: [
                    URLQueryItem(name: "id", value: item.id),
                    URLQueryItem(name: "path", value: item.path),
                ]
            )
            details[item.key] = detail
            return detail
        } catch {
            return nil
        }
    }

    func item(at index: Int) -> SkeletonItem? {
        let items = displayItems
        return items.indices.contains(index) ? items[index] : nil
    }

    // MARK: - Request building

    private func request(for view: LibraryView, query: QueryState) -> ItemListRequest {
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
}

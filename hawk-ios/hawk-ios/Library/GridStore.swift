import Foundation
import Observation

/// 单个网格页面的浏览状态：视图、查询、骨架、分页详情与锁定态。
/// 库级数据（分类树、计数、锁集、排序偏好）在 `LibraryContext`，同会话的多个网格共享。
@MainActor
@Observable
final class GridStore {
    let context: LibraryContext

    private(set) var view: LibraryView
    var query = QueryState()
    var searchText = ""

    private(set) var skeleton: [SkeletonItem] = []
    /// 首屏分页内容：完整骨架到达前先渲染
    private(set) var provisionalSkeleton: [SkeletonItem] = []
    private(set) var details: [String: Item] = [:]
    private(set) var totalSize: Int64 = 0
    private(set) var loading = false
    private(set) var firstPageLoading = false
    private(set) var error: String?
    private(set) var lockedTarget: LockTarget?
    private(set) var skeletonVersion = 0
    /// 布局版本：分页内容、完整骨架、清空都会推进，驱动网格重排
    private(set) var layoutVersion = 0
    /// 滚动位置记忆（按行首 item key）
    var scrollAnchor: String?

    private var reloadTask: Task<Void, Never>?
    private var searchTask: Task<Void, Never>?
    private var contentTask: Task<Void, Never>?
    private var loadedPages: Set<Int> = []
    private var pendingPages: Set<Int> = []
    private let pageSize = 48

    init(context: LibraryContext, view: LibraryView) {
        self.context = context
        self.view = view
        self.scrollAnchor = context.scrollAnchors[view.id]
        applySortPreference()
    }

    /// 滚动位置记忆：切走再回来时恢复到同一行
    func updateScrollAnchor(_ key: String?) {
        guard scrollAnchor != key else { return }
        scrollAnchor = key
        if let key {
            context.scrollAnchors[view.id] = key
        }
    }

    var displayItems: [SkeletonItem] { skeleton.isEmpty ? provisionalSkeleton : skeleton }
    var total: Int { displayItems.count }
    var currentSort: SortChoice? { SortChoice.current(query) }
    var hasActiveFilters: Bool { query.hasActiveFilters }
    var viewTitle: String { view.title }

    var emptyDescription: String {
        if hasActiveFilters {
            return "当前筛选条件下没有素材，试试清除筛选。"
        }
        if !query.keywords.isEmpty {
            return "没有匹配「\(query.keywords.joined(separator: " "))」的素材。"
        }
        switch view {
        case .trash: return "回收站为空。"
        case .uncategorized: return "所有素材都已分类。"
        case .untagged: return "所有素材都已有标签。"
        case .folder: return "这个文件夹里还没有素材。"
        default: return "在桌面端导入素材后，这里会自动出现。"
        }
    }

    // MARK: - 启动与刷新

    /// 进入页面时调用：首次加载或沿用已有内容
    func startIfNeeded() {
        guard skeleton.isEmpty, provisionalSkeleton.isEmpty, error == nil, lockedTarget == nil
        else { return }
        load()
    }

    /// 发起加载：锁定视图不发请求，直接切解锁界面
    func load() {
        if let locked = context.lockTarget(for: view), !context.isUnlocked(view) {
            lockedTarget = locked
            loading = false
            clearContent()
            return
        }
        lockedTarget = nil
        contentTask?.cancel()
        contentTask = Task { [weak self] in
            guard let self else { return }
            async let preview: Void = self.loadFirstPagePreview()
            async let full: Void = self.loadSkeleton()
            _ = await (preview, full)
        }
    }

    /// SSE / 解锁 / 隐藏集变化：内容可能已变化，重查当前视图
    func handleStructureChange() {
        reloadTask?.cancel()
        reloadTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 400_000_000)
            guard !Task.isCancelled, let self else { return }
            self.load()
        }
    }

    private func loadFirstPagePreview() async {
        guard skeleton.isEmpty, provisionalSkeleton.isEmpty else { return }
        let viewSnapshot = view
        let querySnapshot = query
        firstPageLoading = true
        defer { firstPageLoading = false }

        var request = context.request(for: viewSnapshot, query: querySnapshot)
        request.offset = 0
        request.limit = pageSize
        do {
            let response: ItemListResponse = try await context.api.post(
                "/api/v1/item/list", request)
            guard !Task.isCancelled, skeleton.isEmpty, view == viewSnapshot,
                query == querySnapshot
            else { return }
            guard !response.items.isEmpty else { return }
            for item in response.items {
                details[item.key] = item
            }
            provisionalSkeleton = response.items.map {
                SkeletonItem(
                    id: $0.id, path: $0.path, width: $0.width, height: $0.height, star: $0.star,
                    size: $0.size)
            }
            totalSize = response.totalSize
            layoutVersion += 1
        } catch {
            // 首屏分页失败静默：完整骨架接管
            handleLoadFailure(error)
        }
    }

    private func loadSkeleton() async {
        skeletonVersion += 1
        let version = skeletonVersion
        loading = true
        error = nil
        defer {
            if version == skeletonVersion { loading = false }
        }

        let viewSnapshot = view
        let querySnapshot = query
        let request = context.request(for: viewSnapshot, query: querySnapshot)
        do {
            let response: ItemSkeletonResponse = try await context.api.post(
                "/api/v1/item/skeleton", request)
            guard version == skeletonVersion, view == viewSnapshot, query == querySnapshot else {
                return
            }
            skeleton = response.items
            totalSize = response.totalSize
            provisionalSkeleton.removeAll()
            layoutVersion += 1
            let validKeys = Set(response.items.map(\.key))
            for key in details.keys where !validKeys.contains(key) {
                details.removeValue(forKey: key)
            }
            loadedPages.removeAll()
            pendingPages.removeAll()
            // 首屏分页已覆盖第一页时不再重复请求 item/list
            if !response.items.isEmpty,
                response.items.prefix(pageSize).allSatisfy({ details[$0.key] != nil })
            {
                loadedPages.insert(0)
            }
            ensureWindow(0..<min(pageSize, response.items.count))
        } catch {
            guard version == skeletonVersion else { return }
            if error is CancellationError { return }
            handleLoadFailure(error)
        }
    }

    private func handleLoadFailure(_ error: any Error) {
        if let apiError = error as? APIError, apiError.isLocked {
            // 锁状态可能刚变化：以服务端判定为准
            lockedTarget =
                context.lockTarget(for: view)
                ?? view.lockTarget.map { LockTarget(dimension: $0.dimension, name: $0.name) }
            clearContent()
            return
        }
        self.error = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }

    private func clearContent() {
        skeleton = []
        provisionalSkeleton = []
        details.removeAll(keepingCapacity: true)
        totalSize = 0
        skeletonVersion += 1
        layoutVersion += 1
        loadedPages.removeAll()
        pendingPages.removeAll()
        error = nil
        loading = false
    }

    // MARK: - 视图 / 查询

    func select(view: LibraryView) {
        guard self.view != view else { return }
        self.view = view
        scrollAnchor = nil
        applySortPreference()
        clearContent()
        loading = true
        searchText = ""
        query.keywords = []
        load()
    }

    /// 返回上级：文件夹回父目录（根目录回全部素材），其他维度回全部素材。
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
            try? await Task.sleep(nanoseconds: 350_000_000)
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
        clearContent()
        loading = true
        load()
    }

    func setSort(_ choice: SortChoice) {
        guard query.orderBy != choice.orderBy || query.order != choice.order else { return }
        query.orderBy = choice.orderBy
        query.order = choice.order
        context.saveSortPreference(query.sort, for: view)
        clearContent()
        loading = true
        load()
    }

    func applyFilters(star: Int?, color: String?, size: SizeFilter?) {
        guard query.star != star || query.color != color || query.size != size else { return }
        query.star = star
        query.color = color
        query.size = size
        clearContent()
        loading = true
        load()
    }

    func clearFilters(keepingSearch: Bool = true) {
        if !keepingSearch { searchText = "" }
        applyFilters(star: nil, color: nil, size: nil)
    }

    private func applySortPreference() {
        guard let preference = context.sortPreference(for: view) else { return }
        query.apply(preference)
    }

    // MARK: - 详情

    func detail(for item: SkeletonItem) -> Item? {
        context.updatedItems[item.key] ?? details[item.key]
    }

    /// 预览需要单条完整元数据（标签/分类/备注）：优先窗口缓存，否则按 id + path 拉一次。
    func detail(at index: Int) async -> Item? {
        let items = displayItems
        guard items.indices.contains(index) else { return nil }
        let item = items[index]
        if let cached = detail(for: item) { return cached }
        do {
            let detail: Item = try await context.api.get(
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

    func star(for item: SkeletonItem) -> Int {
        detail(for: item)?.star ?? item.star
    }

    func item(at index: Int) -> SkeletonItem? {
        let items = displayItems
        return items.indices.contains(index) ? items[index] : nil
    }

    // MARK: - 分页详情

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
                if self.skeletonVersion == version { self.pendingPages.remove(key) }
            }
            var request = self.context.request(for: self.view, query: self.query)
            request.offset = page * self.pageSize
            request.limit = self.pageSize
            do {
                let response: ItemListResponse = try await self.context.api.post(
                    "/api/v1/item/list", request)
                guard self.skeletonVersion == version else { return }
                for item in response.items {
                    self.details[item.key] = item
                }
                self.loadedPages.insert(key)
            } catch {
                // 视口窗口失败静默，后续 onAppear/刷新重试
            }
        }
    }
}

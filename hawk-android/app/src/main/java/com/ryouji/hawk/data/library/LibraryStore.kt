package com.ryouji.hawk.data.library

import com.ryouji.hawk.data.model.FolderNode
import com.ryouji.hawk.data.model.GlobalFilterSnapshot
import com.ryouji.hawk.data.model.Item
import com.ryouji.hawk.data.model.ItemKey
import com.ryouji.hawk.data.model.ItemListRequest
import com.ryouji.hawk.data.model.ItemListResponse
import com.ryouji.hawk.data.model.ItemSkeletonResponse
import com.ryouji.hawk.data.model.LibraryCounts
import com.ryouji.hawk.data.model.LibraryInfo
import com.ryouji.hawk.data.model.LibraryView
import com.ryouji.hawk.data.model.QueryState
import com.ryouji.hawk.data.model.SizeFilter
import com.ryouji.hawk.data.model.SkeletonItem
import com.ryouji.hawk.data.model.SortChoice
import com.ryouji.hawk.data.model.TaxonInfo
import com.ryouji.hawk.data.net.ApiClient
import kotlin.math.max
import kotlin.math.min
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class LibraryUiState(
    val library: LibraryInfo? = null,
    val folders: FolderNode? = null,
    val categories: List<TaxonInfo> = emptyList(),
    val tags: List<TaxonInfo> = emptyList(),
    val globalFilter: GlobalFilterSnapshot = GlobalFilterSnapshot(),
    val counts: LibraryCounts = LibraryCounts(),
    val view: LibraryView = LibraryView.All,
    val query: QueryState = QueryState(),
    val searchText: String = "",
    val skeleton: List<SkeletonItem> = emptyList(),
    val provisional: List<SkeletonItem> = emptyList(),
    val details: Map<String, Item> = emptyMap(),
    val totalSize: Long = 0,
    val loading: Boolean = false,
    val error: String? = null,
    val skeletonVersion: Int = 0,
    val layoutVersion: Int = 0,
    val taxonomyLoading: Boolean = true,
    val countsLoading: Boolean = true,
    val firstPageLoading: Boolean = false,
) {
    /** 当前实际展示的骨架：优先完整骨架；骨架未到时用首屏分页内容占位。 */
    val displayItems: List<SkeletonItem> get() = if (skeleton.isEmpty()) provisional else skeleton
    val total: Int get() = displayItems.size
    val currentSort: SortChoice? get() = SortChoice.current(query)
    val viewTitle: String get() = view.title
}

/**
 * 一个已连接素材库的浏览状态。
 *
 * 在线首屏策略（与 iOS 一致）：bootstrap 只等 library/info 与 global_filter，
 * 之后并发拉首屏分页、完整骨架、分类维度，计数最后补。
 */
class LibraryStore(
    val api: ApiClient,
    private val scope: CoroutineScope,
) {
    private val _state = MutableStateFlow(LibraryUiState())
    val state: StateFlow<LibraryUiState> = _state.asStateFlow()

    private var reloadJob: Job? = null
    private var searchJob: Job? = null
    private var contentJob: Job? = null
    private var lastLoadedAt = 0L
    private val loadedPages = mutableSetOf<Int>()
    private val pendingPages = mutableSetOf<Int>()
    private val pageSize = 48

    val hasActiveFilters: Boolean get() = _state.value.query.hasActiveFilters

    // MARK: - Bootstrap

    /** 只做「能出界面」的最小准备：库信息 + 全局隐藏集；其余在后台补齐。 */
    suspend fun bootstrap() {
        val filter = coroutineScope {
            val filterTask = async {
                runCatching { api.get<GlobalFilterSnapshot>("/api/v1/global_filter/list") }
                    .getOrNull()
            }
            val library = api.get<LibraryInfo>("/api/v1/library/info")
            _state.update { it.copy(library = library, globalFilter = filterTask.await() ?: it.globalFilter) }
        }
        startDeferredLoad()
    }

    private fun startDeferredLoad() {
        contentJob?.cancel()
        _state.update { it.copy(loading = true) }
        contentJob = scope.launch {
            coroutineScope {
                launch { loadContentWithPreview() }
                launch { refreshTaxonomy() }
            }
            refreshCounts()
        }
    }

    private suspend fun loadContentWithPreview() {
        coroutineScope {
            launch { loadFirstPagePreview() }
            launch { loadSkeletonNow() }
        }
    }

    /** 回到前台时刷新（节流）：浏览期间素材可能在桌面端被增删改。 */
    suspend fun refreshIfStale(minIntervalMs: Long = 120_000) {
        if (System.currentTimeMillis() - lastLoadedAt <= minIntervalMs) return
        refreshAll()
    }

    suspend fun refreshAll() {
        coroutineScope {
            launch { refreshTaxonomy() }
            launch { refreshCounts() }
            launch { loadContentWithPreview() }
        }
    }

    private suspend fun refreshTaxonomy() {
        _state.update { it.copy(taxonomyLoading = true) }
        try {
            coroutineScope {
                val foldersTask = async { api.get<FolderNode>("/api/v1/folder/list") }
                val categoriesTask = async { api.get<List<TaxonInfo>>("/api/v1/category/list") }
                val tagsTask = async { api.get<List<TaxonInfo>>("/api/v1/tag/list") }
                val folders = foldersTask.await()
                val categories = categoriesTask.await()
                val tags = tagsTask.await()
                val filter = runCatching { api.get<GlobalFilterSnapshot>("/api/v1/global_filter/list") }
                    .getOrNull()
                _state.update {
                    it.copy(
                        folders = folders,
                        categories = categories,
                        tags = tags,
                        globalFilter = filter ?: it.globalFilter,
                    )
                }
            }
        } catch (error: CancellationException) {
            throw error
        } catch (_: Exception) {
            // 分类维度刷新失败不打断主列表
        } finally {
            _state.update { it.copy(taxonomyLoading = false) }
        }
    }

    suspend fun refreshCounts() {
        _state.update { it.copy(countsLoading = true) }
        try {
            val counts = coroutineScope {
                val all = async { count(LibraryView.All) }
                val root = async { count(LibraryView.Root) }
                val uncategorized = async { count(LibraryView.Uncategorized) }
                val untagged = async { count(LibraryView.Untagged) }
                val trash = async { count(LibraryView.Trash) }
                LibraryCounts(
                    all = all.await(),
                    root = root.await(),
                    uncategorized = uncategorized.await(),
                    untagged = untagged.await(),
                    trash = trash.await(),
                )
            }
            _state.update { it.copy(counts = counts) }
        } catch (error: CancellationException) {
            throw error
        } finally {
            _state.update { it.copy(countsLoading = false) }
        }
    }

    private suspend fun count(view: LibraryView): Int = try {
        val request = request(view, QueryState()).copy(limit = 1)
        val response: ItemListResponse = api.post("/api/v1/item/list", request)
        response.total
    } catch (error: CancellationException) {
        throw error
    } catch (_: Exception) {
        0
    }

    // MARK: - 视图 / 查询

    fun select(view: LibraryView) {
        if (_state.value.view == view) return
        _state.update { it.copy(view = view) }
        scheduleReload()
    }

    /** 返回上层：文件夹回到父目录（根目录回全部素材），其他维度回到全部素材。 */
    fun navigateUp() {
        when (val view = _state.value.view) {
            is LibraryView.Folder -> {
                val path = view.path
                val slash = path.lastIndexOf('/')
                select(if (slash > 0) LibraryView.Folder(path.substring(0, slash)) else LibraryView.Root)
            }

            else -> select(LibraryView.All)
        }
    }

    fun setSearchText(text: String) {
        _state.update { it.copy(searchText = text) }
        searchJob?.cancel()
        searchJob = scope.launch {
            delay(420)
            applySearch()
        }
    }

    fun submitSearch() {
        searchJob?.cancel()
        applySearch()
    }

    private fun applySearch() {
        val keywords = _state.value.searchText.split(' ', '\n', '\t')
            .map { it.trim() }
            .filter { it.isNotEmpty() }
        if (keywords == _state.value.query.keywords) return
        _state.update { it.copy(query = it.query.copy(keywords = keywords)) }
        scheduleReload()
    }

    fun setSort(choice: SortChoice) {
        val query = _state.value.query
        if (query.orderBy == choice.orderBy && query.order == choice.order) return
        _state.update {
            it.copy(query = it.query.copy(orderBy = choice.orderBy, order = choice.order))
        }
        scheduleReload()
    }

    fun setStar(star: Int?) {
        _state.update { it.copy(query = it.query.copy(star = star)) }
        scheduleReload()
    }

    fun setSize(size: SizeFilter?) {
        _state.update { it.copy(query = it.query.copy(size = size)) }
        scheduleReload()
    }

    fun setColor(color: String?) {
        _state.update { it.copy(query = it.query.copy(color = color)) }
        scheduleReload()
    }

    fun clearFilters() {
        _state.update {
            it.copy(query = it.query.copy(star = null, color = null, size = null))
        }
        scheduleReload()
    }

    /** 筛选面板一次性应用全部条件（避免每个维度各触发一轮骨架请求）。 */
    fun applyFilters(star: Int?, color: String?, size: SizeFilter?) {
        _state.update { it.copy(query = it.query.copy(star = star, color = color, size = size)) }
        scheduleReload()
    }

    /** 空态里一次清掉搜索与全部筛选。 */
    fun clearFiltersAndSearch() {
        _state.update {
            it.copy(
                searchText = "",
                query = it.query.copy(
                    keywords = emptyList(),
                    star = null,
                    color = null,
                    size = null,
                ),
            )
        }
        scheduleReload()
    }

    // MARK: - 骨架

    /** 首屏分页：完整骨架到达前先显示第一页素材，让内容尽早出现。 */
    private suspend fun loadFirstPagePreview() {
        val snapshot = _state.value
        if (snapshot.skeleton.isNotEmpty() || snapshot.provisional.isNotEmpty()) return
        val viewSnapshot = snapshot.view
        val querySnapshot = snapshot.query
        _state.update { it.copy(firstPageLoading = true) }
        try {
            val request = request(viewSnapshot, querySnapshot).copy(offset = 0, limit = pageSize)
            val response: ItemListResponse = api.post("/api/v1/item/list", request)
            val current = _state.value
            if (current.skeleton.isNotEmpty() || current.view != viewSnapshot || current.query != querySnapshot) {
                return
            }
            if (response.items.isEmpty()) return
            _state.update { state ->
                state.copy(
                    provisional = response.items.map { item ->
                        SkeletonItem(
                            id = item.id,
                            path = item.path,
                            width = item.width,
                            height = item.height,
                            star = item.star,
                            size = item.size,
                        )
                    },
                    details = state.details + response.items.associateBy { it.key },
                    totalSize = response.totalSize,
                    layoutVersion = state.layoutVersion + 1,
                )
            }
        } catch (error: CancellationException) {
            throw error
        } catch (_: Exception) {
            // 首屏失败静默：完整骨架 / 下拉刷新会接管
        } finally {
            _state.update { it.copy(firstPageLoading = false) }
        }
    }

    suspend fun reload() {
        loadSkeletonNow()
    }

    private fun scheduleReload(debounceMilliseconds: Long = 0) {
        clearQueryContent()
        // 只重取内容：分类/标签与计数是全局维度，不随视图/筛选变化；首屏后台任务继续跑，
        // 旧请求靠 skeletonVersion / 视图快照自行作废。
        reloadJob?.cancel()
        reloadJob = scope.launch {
            if (debounceMilliseconds > 0) delay(debounceMilliseconds)
            loadContentWithPreview()
        }
    }

    /** 查询条件切换时的内容重置：旧骨架/详情/分页状态全部作废。 */
    private fun clearQueryContent() {
        loadedPages.clear()
        pendingPages.clear()
        _state.update {
            it.copy(
                skeleton = emptyList(),
                provisional = emptyList(),
                details = emptyMap(),
                totalSize = 0,
                skeletonVersion = it.skeletonVersion + 1,
                layoutVersion = it.layoutVersion + 1,
                error = null,
                loading = true,
            )
        }
    }

    private suspend fun loadSkeletonNow() {
        val version = _state.value.skeletonVersion + 1
        _state.update { it.copy(skeletonVersion = version, loading = true, error = null) }
        try {
            val request = request(_state.value.view, _state.value.query)
            val response: ItemSkeletonResponse = api.post("/api/v1/item/skeleton", request)
            if (_state.value.skeletonVersion != version) return
            val validKeys = response.items.mapTo(HashSet()) { it.key }
            _state.update { state ->
                state.copy(
                    skeleton = response.items,
                    totalSize = response.totalSize,
                    provisional = emptyList(),
                    layoutVersion = state.layoutVersion + 1,
                    details = state.details.filterKeys { it in validKeys },
                    loading = false,
                )
            }
            lastLoadedAt = System.currentTimeMillis()
            loadedPages.clear()
            pendingPages.clear()
            val firstPage = response.items.take(pageSize)
            if (firstPage.isNotEmpty() && firstPage.all { _state.value.details.containsKey(it.key) }) {
                loadedPages.add(0)
            }
            ensureWindow(0, min(pageSize, response.items.size))
        } catch (error: CancellationException) {
            throw error
        } catch (error: Exception) {
            if (_state.value.skeletonVersion == version) {
                _state.update { it.copy(loading = false, error = error.message ?: "加载素材失败") }
            }
        }
    }

    // MARK: - 详情窗口

    fun ensureWindow(start: Int, endExclusive: Int) {
        val items = _state.value.skeleton
        if (items.isEmpty() || start >= endExclusive) return
        val lower = max(0, start - 12)
        val upper = min(items.size, endExclusive + 12)
        if (lower >= upper) return
        val firstPage = lower / pageSize
        val lastPage = (upper - 1) / pageSize
        for (page in firstPage..lastPage) loadPage(page)
    }

    private fun loadPage(page: Int) {
        if (page < 0) return
        if (page in loadedPages || page in pendingPages) return
        val version = _state.value.skeletonVersion
        val view = _state.value.view
        val query = _state.value.query
        pendingPages.add(page)
        scope.launch {
            try {
                val request = request(view, query).copy(offset = page * pageSize, limit = pageSize)
                val response: ItemListResponse = api.post("/api/v1/item/list", request)
                if (_state.value.skeletonVersion != version) return@launch
                _state.update { state ->
                    state.copy(details = state.details + response.items.associateBy { it.key })
                }
                loadedPages.add(page)
            } catch (error: CancellationException) {
                throw error
            } catch (_: Exception) {
                // 视口窗口失败静默，后续 onAppear / 刷新会重试
            } finally {
                if (_state.value.skeletonVersion == version) pendingPages.remove(page)
            }
        }
    }

    /** 预览需要单条完整元数据：优先用窗口缓存，否则按 id + path 拉一次。 */
    suspend fun detail(index: Int): Item? {
        val item = _state.value.displayItems.getOrNull(index) ?: return null
        _state.value.details[item.key]?.let { return it }
        return try {
            val detail: Item = api.get(
                "/api/v1/item/detail",
                listOf("id" to item.id, "path" to item.path),
            )
            _state.update { it.copy(details = it.details + (detail.key to detail)) }
            detail
        } catch (error: CancellationException) {
            throw error
        } catch (_: Exception) {
            null
        }
    }

    fun item(index: Int): SkeletonItem? = _state.value.displayItems.getOrNull(index)

    // MARK: - 请求构造

    private fun request(view: LibraryView, query: QueryState): ItemListRequest {
        var request = ItemListRequest(orderBy = query.orderBy, order = query.order)
        request = request.copy(
            keywords = query.keywords.takeIf { it.isNotEmpty() },
            star = query.star,
            color = query.color,
        )
        (query.size as? SizeFilter.Side)?.let { size ->
            request = request.copy(minSideGte = size.min, minSideLte = size.max)
        }

        request = when (view) {
            LibraryView.All -> request
            LibraryView.Root -> request.copy(folders = listOf(""), foldersExact = true)
            LibraryView.Uncategorized -> request.copy(withoutCategories = true)
            LibraryView.Untagged -> request.copy(withoutTags = true)
            LibraryView.Trash -> request.copy(inTrash = true)
            is LibraryView.Folder -> request.copy(folders = listOf(view.path))
            is LibraryView.Category -> request.copy(categories = listOf(view.name))
            is LibraryView.Tag -> request.copy(tags = listOf(view.name))
        }

        if (view.isGlobal) {
            val filter = _state.value.globalFilter
            if (filter.folders.isNotEmpty()) request = request.copy(excludeFolders = filter.folders)
            if (filter.categories.isNotEmpty()) {
                request = request.copy(excludeCategories = filter.categories)
            }
            if (filter.tags.isNotEmpty()) request = request.copy(excludeTags = filter.tags)
        }
        return request
    }
}

/** 展示名：优先详情，回退到路径。 */
fun displayTitle(item: SkeletonItem, detail: Item?): String {
    if (detail != null) return detail.displayName
    val name = ItemKey.baseName(item.path)
    val ext = ItemKey.ext(item.path)
    return if (ext.isEmpty()) name else "$name.$ext"
}

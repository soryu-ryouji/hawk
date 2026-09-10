// Pinia 主 store：视图/查询/列表/选择集/回收站与 SSE 事件编排（applyEvent）。
// 组件不直接调 api，读经本 store，写经 ./actions（服务层）；跨域经 index.ts 出口。
// SSE 事件经 applyEvent 分发（分类维度分支转发 taxonomy hooks，组件层编排）。
import { computed, ref, watch } from 'vue';
import { defineStore } from 'pinia';
import { useMediaQuery } from '@vueuse/core';
import { api } from '@/shared/api/endpoints';
import { createViewNavigation, type ViewValidators } from './navigation';
import {
  isGlobalViewKind,
  isUnfilteredView,
  indexSkeletonById,
  itemKey,
  locationSetChangedOf,
  mergeDetailOnUpdate,
  nextSelection,
  patchSkeletonOnUpdate,
  selectionUniqueIds,
  shouldReloadOnUpdate,
  splitKey,
  taxonomyChanged,
} from './logic/viewLogic';
import { hasShell } from '@/shared/lib/platform';
import { loadText, saveText, STORAGE_KEYS } from '@/shared/lib/persist';
import { debounce, errorText } from '@/shared/lib/storeUtil';
import type { GlobalFilter, Item, ItemListRequest, LibraryInfo, QueryState, SkeletonItem, ViewPrefs, ViewState } from '@/shared/types';

/** 首屏窗口大小（条目数）：覆盖首屏 + 少量预取；之后按视口区间补数据 */
const INITIAL_WINDOW = 150;

/** 分类维度刷新钩子：taxonomy store 创建时注册（子 → 主方向 import，主 store 不反向引用 taxonomy）。
 *  两个入口内部均防抖——SSE 事件爆发期合并刷新；本地批量操作后调用同样收敛到 300ms 内一次 */
export interface TaxonomyHooks {
  /** 分类/标签集合或成员计数可能变化 → 防抖刷新计数与词表 */
  refreshTaxonomy(): void;
  /** 目录结构变化 → 防抖刷新文件夹树 */
  refreshFolders(): void;
  /** 全局列表隐藏集变更（SSE 负载为完整快照）→ 更新隐藏集并重查列表 */
  onGlobalFilterChanged(filter: GlobalFilter): void;
}
let taxonomyHooks: TaxonomyHooks | null = null;
export function registerTaxonomyHooks(hooks: TaxonomyHooks): void {
  taxonomyHooks = hooks;
}

export type { ViewValidators } from './navigation';

export const useLibraryStore = defineStore('library', () => {
  // ---- state ----
  const view = ref<ViewState>({ kind: 'all' });
  const query = ref<QueryState>({ keywords: [], orderBy: 'modification_time', order: 'desc' });
  /** 当前视图全量骨架（id/path/width/height/star，与 item/list 同查询同排序）：布局与滚动条总高的唯一依据。
   *  同内容（同 hash）多位置各自成条，条目唯一标识为 itemKey(id, path)，selection/details 均以它为键 */
  const skeleton = ref<SkeletonItem[]>([]);
  /** 已拉取的详情（视口窗口 + 预取），按条目 key 索引；不在视口的行只留骨架占位不渲染 */
  const details = ref(new Map<string, Item>());
  /** 当前视图（含筛选）未分页的全量字节数合计，检查器「分区状态」用 */
  const totalSize = ref(0);
  /** 整表（骨架）加载中 */
  const loading = ref(false);
  /** 视口窗口补数据中 */
  const windowLoading = ref(false);
  /** 骨架版本：换视图/骨架重载时自增，过期窗口响应据此丢弃 */
  let skeletonVersion = 0;
  const selection = ref<string[]>([]);
  /** 选中集的 O(1) 成员查询（渲染层一律用 has，不直接扫数组：
   *  全选数万条目时，逐卡片 includes 是 O(选中数×可见卡片数)，
   *  且响应式会跟踪数组的每个索引——每个卡片的渲染 effect 订阅上万个依赖） */
  const selectionSet = computed(() => new Set(selection.value));
  /** 骨架的 条目key → 字节数 索引（选择集大小聚合的数据源；随骨架替换重建，选中变化不重建） */
  const skeletonSizeMap = computed(() => new Map(skeleton.value.map((s) => [itemKey(s.id, s.path), Number(s.size)])));
  /** 骨架按内容 id 的索引（item.updated 事件处理 O(1) 定位；随骨架替换重建） */
  const skeletonIndexMap = computed(() => indexSkeletonById(skeleton.value));

  /** 选择集的共有特性（标签/分类交集）：多选面板数据源。
   *  详情缓存只覆盖视口窗口，交集由服务端 item/aggregate 全量计算；选择集 ≤1 时为 null */
  const selectionAggregate = ref<{ tags: string[]; categories: string[] } | null>(null);
  let aggregateVersion = 0;
  const debouncedAggregateFetch = debounce(200);

  watch(selection, () => {
    if (selection.value.length > 1) {
      debouncedAggregateFetch(() => void fetchSelectionAggregate());
    } else {
      aggregateVersion++;
      selectionAggregate.value = null;
    }
  });

  /** 拉取选择集聚合（版本守卫丢弃过期响应） */
  async function fetchSelectionAggregate() {
    const version = ++aggregateVersion;
    const ids = selectionUniqueIds(selection.value);
    try {
      const res = await api.itemAggregate(ids);
      if (version === aggregateVersion) {
        selectionAggregate.value = { tags: res.common_tags, categories: res.common_categories };
      }
    } catch {
      // 下次选择变更/批量操作完成时再对齐
    }
  }
  const library = ref<LibraryInfo | null>(null);
  // 网格卡片边长偏好（滑杆 120–280，齐行布局的目标行高）：
  // - 桌面端（Electron）：会话级、固定默认 160，不持久化；
  // - web 端（浏览器，含局域网查看）：用户显式设置过则记忆到 localStorage（`hawk:thumbSize`）
  //   且不再自动切换；未设置时跟随视口宽度的动态默认——宽度足够（≥700px，可并排 3 张
  //   常规横图）用 160 常规网格，不足（手机竖屏等）用最大 280 大图流，横竖屏旋转自动跟随
  const THUMB_SIZE_MIN = 120;
  const THUMB_SIZE_MAX = 280;
  const isBrowserClient = !hasShell;
  const thumbSize = ref(160);
  let userThumbSize: number | null = null;
  if (isBrowserClient) {
    const wideEnough = useMediaQuery('(min-width: 700px)');
    userThumbSize = loadUserThumbSize();
    thumbSize.value = userThumbSize ?? (wideEnough.value ? 160 : THUMB_SIZE_MAX);
    watch(wideEnough, () => {
      if (userThumbSize === null) {
        thumbSize.value = wideEnough.value ? 160 : THUMB_SIZE_MAX;
      }
    });
  }

  function loadUserThumbSize(): number | null {
    const saved = Number(loadText(STORAGE_KEYS.thumbSize));
    return Number.isFinite(saved) && saved >= THUMB_SIZE_MIN && saved <= THUMB_SIZE_MAX ? saved : null;
  }

  /** 用户显式设置缩略图尺寸（设置面板滑杆/± 按钮）：写入偏好并停止跟随动态默认 */
  function setUserThumbSize(size: number) {
    thumbSize.value = Math.min(THUMB_SIZE_MAX, Math.max(THUMB_SIZE_MIN, size));
    if (!isBrowserClient) {
      return;
    }
    userThumbSize = thumbSize.value;
    saveText(STORAGE_KEYS.thumbSize, String(thumbSize.value));
  }
  /** 搜索框草稿（顶栏与检查器顶搜索框共用一份，回车提交为 keywords） */
  const searchText = ref('');
  const toast = ref<string | null>(null);
  /** 缩略图后台积压（task.progress 事件驱动；null 表示无积压，进度条隐藏） */
  const taskBacklog = ref<{ pending: number; active: number } | null>(null);
  /** 索引管道进度（task.progress 事件驱动；扫描期间带阶段进度；null 表示空闲） */
  const indexProgress = ref<{ pending: number; active: number; phase: string | null; processed: number | null; total: number | null } | null>(null);
  const sidebarVisible = ref(true);
  /** 筛选工具列手动展开（评分/颜色等条件激活时条带常驻，见 hasActiveFilters） */
  const filterBarVisible = ref(false);
  /** 视图排序偏好（folder/category/tag 作用域；folder 继承沿父链解析） */
  const viewPrefs = ref<ViewPrefs>({});
  /** 全局列表隐藏集（.hawk/global_filter.toml）：由 taxonomy store 拉取后经 setGlobalFilter 注入
   * （引用方向 DAG：主 store 不反向引用 taxonomy），listParams 在全局类视图附带排除参数 */
  const globalFilter = ref<GlobalFilter>({ folders: [], categories: [], tags: [] });

  // ---- getters ----
  const isTrash = computed(() => view.value.kind === 'trash');
  /** 查询是否带有筛选条件（评分/颜色/尺寸）：有则筛选工具列常驻显示 */
  const hasActiveFilters = computed(() => query.value.star !== undefined || !!query.value.color || query.value.size !== undefined);
  const currentFolderPath = computed(() => (view.value.kind === 'folder' ? view.value.path : null));
  const selectedItems = computed(() => selection.value.map((id) => details.value.get(id)).filter((i): i is Item => !!i));
  const primarySelected = computed(() => selectedItems.value.at(-1) ?? null);
  /** 当前视图名称（检查器「分区状态」标题） */
  const viewTitle = computed(() => {
    const v = view.value;
    if (v.kind === 'all') return '全部素材';
    if (v.kind === 'root') return '根目录素材';
    if (v.kind === 'uncategorized') return '未分类素材';
    if (v.kind === 'untagged') return '未标签素材';
    if (v.kind === 'trash') return '回收站';
    if (v.kind === 'tag' || v.kind === 'category') return v.name;
    return v.path.split('/').pop() ?? '';
  });
  /** 当前视图条目数（= 骨架长度；骨架未加载时为 0） */
  const total = computed(() => skeleton.value.length);
  /** 索引进度条文案（启动屏/主界面共用）：扫描中带阶段进度（遍历阶段总数未知，只报已处理数）；否则报剩余任务数 */
  const indexProgressText = computed(() => {
    const p = indexProgress.value;
    if (!p) {
      return '';
    }
    if (p.phase) {
      const label = p.phase === 'scan' ? '扫描' : p.phase === 'hash' ? '哈希' : '应用';
      const total = p.total ?? 0;
      const processed = p.processed ?? 0;
      return total > 0 ? `正在索引素材库 · ${label} ${processed}/${total}` : `正在索引素材库 · 已发现 ${processed} 个文件`;
    }
    return `正在索引素材 · 剩余 ${p.pending + p.active}`;
  });

  // ---- 内部 ----
  const debouncedSkeletonReload = debounce(200);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  /** 当前视图 + 查询条件的列表参数（不含分页）：骨架与视口窗口共用，保证两次查询次序逐位一致 */
  function listParams(): Omit<ItemListRequest, 'offset' | 'limit'> {
    return {
      keywords: query.value.keywords.length > 0 ? query.value.keywords : undefined,
      star: query.value.star,
      color: query.value.color,
      min_side_gte: query.value.size?.kind === 'side' ? query.value.size.min : undefined,
      min_side_lte: query.value.size?.kind === 'side' ? query.value.size.max : undefined,
      min_width: query.value.size?.kind === 'wh' ? query.value.size.minWidth : undefined,
      max_width: query.value.size?.kind === 'wh' ? query.value.size.maxWidth : undefined,
      min_height: query.value.size?.kind === 'wh' ? query.value.size.minHeight : undefined,
      max_height: query.value.size?.kind === 'wh' ? query.value.size.maxHeight : undefined,
      order_by: query.value.orderBy,
      order: query.value.order,
      in_trash: isTrash.value || undefined,
      ...globalExcludes(),
      folders: view.value.kind === 'folder' ? [view.value.path] : view.value.kind === 'root' ? [''] : undefined,
      folders_exact: view.value.kind === 'root' ? true : undefined,
      without_categories: view.value.kind === 'uncategorized' ? true : undefined,
      without_tags: view.value.kind === 'untagged' ? true : undefined,
      categories: view.value.kind === 'category' ? [view.value.name] : undefined,
      tags: view.value.kind === 'tag' ? [view.value.name] : undefined,
    };
  }

  /** 全局类视图（全部/根目录/未分类/未标签）应用隐藏排除；维度自身视图与回收站不排除 */
  function isGlobalView(): boolean {
    return isGlobalViewKind(view.value);
  }

  /** 隐藏排除激活：全局类视图且隐藏集非空（分类/标签变化可能改变成员资格） */
  function exclusionActive(): boolean {
    const gf = globalFilter.value;
    return isGlobalView() && gf.folders.length + gf.categories.length + gf.tags.length > 0;
  }

  /** 隐藏排除参数：仅全局类视图且隐藏集非空时附带（OR 语义：命中任一隐藏维度即排除） */
  function globalExcludes(): Pick<ItemListRequest, 'exclude_folders' | 'exclude_categories' | 'exclude_tags'> {
    if (!isGlobalView()) {
      return {};
    }
    const gf = globalFilter.value;
    return {
      exclude_folders: gf.folders.length > 0 ? gf.folders : undefined,
      exclude_categories: gf.categories.length > 0 ? gf.categories : undefined,
      exclude_tags: gf.tags.length > 0 ? gf.tags : undefined,
    };
  }

  /** 隐藏集注入（taxonomy store 拉取/接收事件后调用）：全局类视图下成员可能变化 → 重载骨架 */
  function setGlobalFilter(gf: GlobalFilter) {
    globalFilter.value = gf;
  }

  function showToast(message: string) {
    toast.value = message;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast.value = null), 3000);
  }

  // ---- 初始与查询 ----
  /** 局域网 web 查看（viewer token）：默认只读（全部写入口隐藏，服务端 403 为最终防线）；
   * [web] writable 开启后解除只读（app/info.writable 驱动，web 端可上传/删除/修改） */
  const viewerMode = ref(false);

  async function init(validators: ViewValidators) {
    const info = await api.appInfo();
    viewerMode.value = info.access === 'viewer' && !info.writable;
    library.value = await api.libraryInfo();

    // 换库/应用设置重启复用本入口：清掉上一库的会话状态，避免视图/预览/进度指示残留。
    // 视图回退由 restoreView 负责（无记忆时回默认视图），这里只清与库无关的记忆
    query.value = { keywords: [], orderBy: 'modification_time', order: 'desc' };
    searchText.value = '';
    clearSelection();
    // 预览/编辑浮层的会话清理由组件层编排（App.vue runBoot 调 preview store）
    taskBacklog.value = null;
    indexProgress.value = null;

    await loadViewPrefs();
    restoreView(validators);
    applySortForView(view.value); // 恢复的视图应用其记忆的排序
    // 历史栈以恢复后的视图为起点
    nav.resetHistory();
    await resetList();
  }

  // ---- 视图导航与排序偏好：拆到 libraryNavigation（独立状态机 + 依赖注入），此处只做接线 ----
  const nav = createViewNavigation({
    view,
    query,
    viewPrefs,
    viewerMode,
    library,
    clearSelection,
    resetList: () => void resetList(),
    showToast,
  });
  const {
    loadViewPrefs,
    restoreView,
    applySortForView,
    setView,
    correctView,
    goBack,
    goForward,
    canGoBack,
    canGoForward,
    persistSortForCurrentView,
    resetSort,
  } = nav;

  /** Eagle 式侧栏开关：同时显隐左侧栏与右侧检查器 */
  function toggleSidebar() {
    sidebarVisible.value = !sidebarVisible.value;
  }

  /** 筛选工具列开关（TitleBar 漏斗按钮） */
  function toggleFilterBar() {
    filterBarVisible.value = !filterBarVisible.value;
  }

  function setQuery(patch: Partial<QueryState>) {
    Object.assign(query.value, patch);
    if (patch.orderBy !== undefined || patch.order !== undefined) {
      persistSortForCurrentView();
    }
    void resetList();
  }

  /** 搜索框回车提交：按空格拆关键词 */
  function submitSearch() {
    setQuery({ keywords: searchText.value.trim().split(/\s+/).filter(Boolean) });
  }

  async function resetList() {
    const version = ++skeletonVersion;
    loading.value = true;
    try {
      const res = await api.itemSkeleton(listParams());
      if (version !== skeletonVersion) {
        return; // 期间已切视图/重载，结果作废
      }
      skeleton.value = res.items;
      totalSize.value = Number(res.total_size);
      details.value = new Map();
      await ensureWindow(0, INITIAL_WINDOW);
    } catch (e) {
      showToast(errorText(e));
    } finally {
      loading.value = false;
    }
  }

  /**
   * 视口窗口补数据：按骨架索引区间拉 item/list（同查询同排序，偏移与骨架逐位对齐），
   * 区间内已全部缓存则跳过。骨架版本变化时丢弃过期响应。
   */
  async function ensureWindow(start: number, end: number) {
    const sk = skeleton.value;
    const from = Math.max(0, Math.min(start, sk.length));
    const to = Math.max(from, Math.min(end, sk.length));
    let missing = false;
    for (let i = from; i < to; i++) {
      if (!details.value.has(itemKey(sk[i].id, sk[i].path))) {
        missing = true;
        break;
      }
    }
    if (!missing) {
      return;
    }

    const version = skeletonVersion;
    windowLoading.value = true;
    try {
      const res = await api.itemList({ ...listParams(), offset: from, limit: to - from });
      if (version !== skeletonVersion) {
        return;
      }
      const map = new Map(details.value);
      for (const item of res.items) {
        map.set(itemKey(item.id, item.path), item);
      }
      details.value = map;
    } catch {
      // 窗口加载失败静默：后续滚动/重试会再次触发
    } finally {
      windowLoading.value = false;
    }
  }

  /**
   * SSE 驱动的骨架重载：条目增删/成员资格/次序以服务端查询为准；详情缓存保留，
   * 已不属于当前视图的条目就地清理。滚动位置不动，内容就地增删。
   * 事件爆发期合并 in-flight 请求：重载进行中再来事件只置脏标记，当前轮结束后补一轮，
   * 避免高频全量骨架请求占满服务端查询。
   */
  let skeletonReloading = false;
  let skeletonDirty = false;

  async function reloadSkeleton() {
    if (skeletonReloading) {
      skeletonDirty = true;
      return;
    }

    skeletonReloading = true;
    try {
      do {
        skeletonDirty = false;
        await reloadSkeletonOnce();
      } while (skeletonDirty);
    } finally {
      skeletonReloading = false;
    }
  }

  async function reloadSkeletonOnce() {
    const version = ++skeletonVersion;
    try {
      const res = await api.itemSkeleton(listParams());
      if (version !== skeletonVersion) {
        return; // 期间已切视图/重载，结果作废
      }
      skeleton.value = res.items;
      totalSize.value = Number(res.total_size);
      const keys = new Set(res.items.map((i) => itemKey(i.id, i.path)));
      selection.value = selection.value.filter((key) => keys.has(key));
      if (details.value.size > 0) {
        const map = new Map(details.value);
        let changed = false;
        for (const key of [...map.keys()]) {
          if (!keys.has(key)) {
            map.delete(key);
            changed = true;
          }
        }
        if (changed) {
          details.value = map;
        }
      }
    } catch {
      // 下次事件或 SSE 重连再对齐
    }
  }

  // ---- 选择（条目以 itemKey 标识，同内容多位置是独立成员） ----
  function select(key: string, mod?: 'range' | 'toggle') {
    // range/toggle 的区间计算在 viewLogic.nextSelection（纯函数，可单测）
    selection.value = nextSelection(skeleton.value, selection.value, key, mod);
  }

  function selectAll() {
    // 基于骨架（全量），不是仅视口窗口
    selection.value = skeleton.value.map((i) => itemKey(i.id, i.path));
  }

  function clearSelection() {
    selection.value = [];
  }

  /**
   * item.updated 的统一入口（updateItem 响应与 SSE 共用）。本函数只做状态接线，
   * 全部判定（位置集/分类维度变化、详情合并、骨架补丁、重载时机）为 viewLogic.ts 纯函数。
   * 位置分布变化时重拉该 hash 全部缓存位置对齐位置级字段（事件载荷只有主位置口径）。
   */
  function applyUpdatedItem(updated: Item, single: boolean) {
    const indices = skeletonIndexMap.value.get(updated.id) ?? [];
    const skelPaths = indices.map((i) => skeleton.value[i].path);
    const locationSetChanged = locationSetChangedOf(skelPaths, updated);
    const keysOfHash = [...details.value.keys()].filter((k) => splitKey(k).id === updated.id);
    let taxChanged = false;
    if (keysOfHash.length > 0) {
      const map = new Map(details.value);
      const updatedKey = itemKey(updated.id, updated.path);
      for (const key of keysOfHash) {
        const prev = map.get(key)!;
        if (taxonomyChanged(prev, updated)) {
          taxChanged = true;
        }
        map.set(key, mergeDetailOnUpdate(prev, updated, key === updatedKey));
      }
      details.value = map;
      if (locationSetChanged) {
        // 各位置条目重拉对齐位置级字段（name/size/mtime 以 detail 为准）
        for (const key of keysOfHash) {
          void refetchLocation(key);
        }
      }
    }
    const { next, changed: skeletonChanged } = patchSkeletonOnUpdate(skeleton.value, indices, updated);
    if (skeletonChanged) {
      skeleton.value = next;
    }
    if (
      shouldReloadOnUpdate({
        locationSetChanged,
        inSkeleton: indices.length > 0,
        skeletonChanged,
        taxonomyChanged: taxChanged,
        unfiltered: isUnfilteredView(view.value, query.value),
        exclusionActive: exclusionActive(),
        single,
      })
    ) {
      debouncedSkeletonReload(() => void reloadSkeleton());
    }
    if (taxChanged) {
      taxonomyHooks?.refreshTaxonomy();
    }
  }

  /** 按条目 key 重拉位置级详情（applyUpdatedItem 检测到位置集变化后调用） */
  async function refetchLocation(key: string) {
    const { id, path } = splitKey(key);
    try {
      const item = await api.itemDetail(id, path);
      if (details.value.has(key)) {
        const map = new Map(details.value);
        map.set(key, item);
        details.value = map;
      }
    } catch {
      // 位置可能已不存在（改名/删除竞态）：交给骨架重载收敛
    }
  }

  /** 防抖骨架重载的公共入口（服务层写操作与 SSE 事件同一条防抖通道，服务层唯一可触发的重载口） */
  function requestSkeletonReload() {
    debouncedSkeletonReload(() => void reloadSkeleton());
  }

  /** 立即重拉选择集聚合（批量写改变共有特性后，不等选择变化防抖） */
  function refreshSelectionAggregate() {
    void fetchSelectionAggregate();
  }

  // ---- SSE ----
  // 事件与副作用的对应关系（不无条件全刷，后台事件爆发期不制造请求风暴）：
  // - item.updated / items.updated：详情/骨架就地更新
  // - item.added / items.added / restored / trashed / removed：成员与计数以服务端查询为准 → 防抖重载骨架 + 刷分类计数
  //   （items.added 为扫描导入的批量合并事件，避免逐条事件风暴）；计数/文件夹树刷新经 TaxonomyHooks 转发 taxonomy store
  // - folder.changed：只刷文件夹树（目录结构变化的唯一信号）
  // - task.progress：只更新对应的后台任务指示
  function applyEvent(type: string, payload: unknown) {
    switch (type) {
      case 'item.updated': {
        applyUpdatedItem(payload as Item, true);
        break;
      }
      case 'items.updated': {
        for (const item of payload as Item[]) {
          applyUpdatedItem(item, false);
        }
        break;
      }
      case 'items.added':
      case 'item.added':
      case 'item.restored':
        // 新 item 的落点（成员/次序）只能以服务端查询为准：防抖重载骨架，视口窗口随后按需补齐
        debouncedSkeletonReload(() => void reloadSkeleton());
        taxonomyHooks?.refreshTaxonomy();
        taxonomyHooks?.refreshFolders(); // 文件增删改变目录计数（含「全部素材」徽章）
        break;
      case 'item.trashed':
      case 'item.removed': {
        const id = (payload as { id: string }).id;
        // 就地移除该 hash 的全部位置条目（立即反馈）；回收站视图同事件意味着「进来了」，统一以防抖重载兜底
        if ([...details.value.keys()].some((k) => splitKey(k).id === id)) {
          const map = new Map(details.value);
          for (const key of [...map.keys()]) {
            if (splitKey(key).id === id) {
              map.delete(key);
            }
          }
          details.value = map;
        }
        if (skeleton.value.some((s) => s.id === id)) {
          skeleton.value = skeleton.value.filter((s) => s.id !== id);
        }
        selection.value = selection.value.filter((key) => splitKey(key).id !== id);
        debouncedSkeletonReload(() => void reloadSkeleton());
        taxonomyHooks?.refreshTaxonomy();
        taxonomyHooks?.refreshFolders();
        break;
      }
      case 'task.progress': {
        const p = payload as { task: string; pending: number; active: number; phase?: string; processed?: number; total?: number };
        // 积压归零撤掉指示;其余帧更新计数(节流由服务端 500ms 保证)
        if (p.task === 'thumbnail') {
          taskBacklog.value = p.pending + p.active > 0 ? { pending: p.pending, active: p.active } : null;
        } else if (p.task === 'index') {
          indexProgress.value =
            p.pending + p.active > 0
              ? { pending: p.pending, active: p.active, phase: p.phase ?? null, processed: p.processed ?? null, total: p.total ?? null }
              : null;
        }
        break;
      }
      case 'folder.changed':
        // 目录结构变化（本端操作/外部进程/对账兜底）：重拉文件夹树；与骨架成员和分类/标签计数无关
        taxonomyHooks?.refreshFolders();
        break;
      case 'library.updated':
        // 改库显示名广播（本端 PATCH 的回声或其他客户端发起）：就地对齐库信息
        library.value = payload as LibraryInfo;
        break;
      case 'global_filter.changed':
        // 隐藏集变更（负载为完整快照）：由 taxonomy store 就地替换并联动重查
        taxonomyHooks?.onGlobalFilterChanged(payload as GlobalFilter);
        break;
    }
  }

  return {
    view,
    query,
    skeleton,
    details,
    total,
    totalSize,
    viewTitle,
    loading,
    windowLoading,
    selection,
    selectionSet,
    skeletonSizeMap,
    skeletonIndexMap,
    selectionAggregate,
    library,
    thumbSize,
    setUserThumbSize,
    searchText,
    toast,
    taskBacklog,
    indexProgress,
    indexProgressText,
    sidebarVisible,
    filterBarVisible,
    viewerMode,
    viewPrefs,
    isTrash,
    canGoBack,
    canGoForward,
    currentFolderPath,
    selectedItems,
    primarySelected,
    hasActiveFilters,
    init,
    setView,
    correctView,
    goBack,
    goForward,
    toggleSidebar,
    toggleFilterBar,
    setQuery,
    resetSort,
    submitSearch,
    resetList,
    ensureWindow,
    reloadSkeleton,
    select,
    selectAll,
    clearSelection,
    requestSkeletonReload,
    refreshSelectionAggregate,
    showToast,
    applyEvent,
    setGlobalFilter,
  };
});

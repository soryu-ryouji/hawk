// Pinia 主 store：视图/查询/列表/选择集/回收站，以及列表与 item 写操作 action。
// 组件不直接调 api，一切经 action；SSE 事件经 applyEvent 分发（分类维度分支在 taxonomy store，组件层编排）。
import { computed, ref, watch } from 'vue';
import { defineStore } from 'pinia';
import { useMediaQuery } from '@vueuse/core';
import { api } from '../api/endpoints';
import {
  isGlobalViewKind,
  isUnfilteredView,
  indexSkeletonById,
  itemKey,
  locationSetChangedOf,
  mergeDetailOnUpdate,
  nextSelection,
  patchSkeletonOnUpdate,
  resolveSort,
  shouldReloadOnUpdate,
  splitKey,
  taxonomyChanged,
} from '../viewLogic';
import { hasShell } from '../platform';
import { loadJSON, loadText, saveJSON, saveText, STORAGE_KEYS } from '../persist';
import { debounce, errorText } from './util';
import type { GlobalFilter, Item, ItemListRequest, LibraryInfo, QueryState, SkeletonItem, ViewPrefs, ViewState } from '../types';

/** 首屏窗口大小（条目数）：覆盖首屏 + 少量预取；之后按视口区间补数据 */
const INITIAL_WINDOW = 150;

/** restoreView 的存在性校验（文件夹/分类/标签数据在 taxonomy store，由组件层注入，保持引用方向 DAG） */
export interface ViewValidators {
  folderExists(path: string): boolean;
  categoryExists(name: string): boolean;
  tagExists(name: string): boolean;
}

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
  /** 浏览历史（会话内）：setView 压入，前进/后退在栈内移动 */
  const viewHistory = ref<ViewState[]>([]);
  const historyIndex = ref(-1);
  /** 视图排序偏好（folder/category/tag 作用域；folder 继承沿父链解析） */
  const viewPrefs = ref<ViewPrefs>({});
  /** 全局列表隐藏集（.hawk/global_filter.toml）：由 taxonomy store 拉取后经 setGlobalFilter 注入
   * （引用方向 DAG：主 store 不反向引用 taxonomy），listParams 在全局类视图附带排除参数 */
  const globalFilter = ref<GlobalFilter>({ folders: [], categories: [], tags: [] });

  // ---- getters ----
  const isTrash = computed(() => view.value.kind === 'trash');
  /** 查询是否带有筛选条件（评分/颜色）：有则筛选工具列常驻显示 */
  const hasActiveFilters = computed(() => query.value.star !== undefined || !!query.value.color);
  const canGoBack = computed(() => historyIndex.value > 0);
  const canGoForward = computed(() => historyIndex.value >= 0 && historyIndex.value < viewHistory.value.length - 1);
  const currentFolderPath = computed(() => (view.value.kind === 'folder' ? view.value.path : null));
  const selectedItems = computed(
    () => selection.value.map((id) => details.value.get(id)).filter((i): i is Item => !!i),
  );
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

  // ---- 内部 ----
  const debouncedSkeletonReload = debounce(200);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  /** 当前视图 + 查询条件的列表参数（不含分页）：骨架与视口窗口共用，保证两次查询次序逐位一致 */
  function listParams(): Omit<ItemListRequest, 'offset' | 'limit'> {
    return {
      keywords: query.value.keywords.length > 0 ? query.value.keywords : undefined,
      star: query.value.star,
      color: query.value.color,
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
    viewHistory.value = [view.value];
    historyIndex.value = 0;
    await resetList();
  }

  /** 视图排序偏好：不可用（旧服务端/网络失败）按无记忆处理 */
  async function loadViewPrefs() {
    try {
      viewPrefs.value = await api.viewPreferences();
    } catch {
      // 忽略：保持空表
    }
  }

  /** 视图记忆：按素材库路径存 localStorage（同一台机器多库互不干扰）；键注册表在 persist.ts */
  function viewStorageKey() {
    return STORAGE_KEYS.lastView(library.value?.path ?? '');
  }

  function restoreView(validators: ViewValidators) {
    // 恢复不了（无记忆/目标已删/数据损坏）一律回退全部素材：
    // 换库复用 init 时 view 残留上一库取值，任何 return 路径都必须显式重置
    const fallback: ViewState = { kind: 'all' };
    const parsed = loadJSON<ViewState | null>(viewStorageKey(), null);
    if (parsed === null) {
      view.value = fallback;
      return;
    }
    const valid =
      (parsed.kind !== 'folder' || validators.folderExists(parsed.path)) &&
      (parsed.kind !== 'category' || validators.categoryExists(parsed.name)) &&
      (parsed.kind !== 'tag' || validators.tagExists(parsed.name));
    view.value = valid ? parsed : fallback;
  }

  /** 应用视图：持久化 + 应用记忆排序 + 清选择 + 重查列表（setView/goBack/correctView 的公共收尾） */
  function applyView(v: ViewState) {
    view.value = v;
    saveJSON(viewStorageKey(), v);
    applySortForView(v);
    clearSelection();
    void resetList();
  }

  function sameView(a: ViewState, b: ViewState) {
    return JSON.stringify(a) === JSON.stringify(b);
  }

  /** 用户主动切换视图：截掉前进分支后压入历史 */
  function setView(v: ViewState) {
    if (!sameView(v, view.value)) {
      viewHistory.value = [...viewHistory.value.slice(0, historyIndex.value + 1), v];
      historyIndex.value = viewHistory.value.length - 1;
    }
    applyView(v);
  }

  /** 数据变更引起的当前视图修正（重命名跟随/删除回退）：就地改当前历史条目，不新增 */
  function correctView(v: ViewState) {
    if (historyIndex.value >= 0) {
      viewHistory.value[historyIndex.value] = v;
    }
    applyView(v);
  }

  /** 标题栏前进/后退：在历史栈内移动，不压入新条目 */
  function goHistory(step: 1 | -1) {
    const target = viewHistory.value[historyIndex.value + step];
    if (!target) {
      return;
    }
    historyIndex.value += step;
    applyView(target);
  }

  function goBack() {
    goHistory(-1);
  }

  function goForward() {
    goHistory(1);
  }

  /** Eagle 式侧栏开关：同时显隐左侧栏与右侧检查器 */
  function toggleSidebar() {
    sidebarVisible.value = !sidebarVisible.value;
  }

  /** 筛选工具列开关（TitleBar 漏斗按钮） */
  function toggleFilterBar() {
    filterBarVisible.value = !filterBarVisible.value;
  }

  /** 当前视图的排序偏好 scope；无记忆语义的视图（全部/回收站等）返回 null */
  function sortScopeOf(v: ViewState): string | null {
    if (v.kind === 'folder') return `folder:${v.path}`;
    if (v.kind === 'category') return `category:${v.name}`;
    if (v.kind === 'tag') return `tag:${v.name}`;
    return null;
  }

  /** 应用视图的有效排序（applyView/初始化/恢复默认用；只改排序字段，不动筛选条件；不触发持久化） */
  function applySortForView(v: ViewState) {
    const sort = resolveSort(v, viewPrefs.value);
    query.value.orderBy = sort.orderBy;
    query.value.order = sort.order;
  }

  /** 排序变更持久化到当前视图作用域（fire-and-forget；无记忆语义或 viewer 只读时不写） */
  function persistSortForCurrentView() {
    const scope = sortScopeOf(view.value);
    if (scope === null || viewerMode.value) return;
    const entry = { order_by: query.value.orderBy, order: query.value.order };
    viewPrefs.value = { ...viewPrefs.value, [scope]: entry };
    void api.viewPreferenceSet(scope, entry.order_by, entry.order).catch(() => {
      showToast('排序偏好保存失败');
      void loadViewPrefs(); // 与服务端不一致时回拉对齐
    });
  }

  /** 排序菜单「跟随父级设置/恢复默认排序」：删除当前视图的自有偏好，回到继承/默认 */
  function resetSort() {
    const scope = sortScopeOf(view.value);
    if (scope === null || viewerMode.value || !(scope in viewPrefs.value)) return;
    const next = { ...viewPrefs.value };
    delete next[scope];
    viewPrefs.value = next;
    applySortForView(view.value);
    void resetList();
    void api.viewPreferenceReset(scope).catch(() => {
      showToast('排序偏好保存失败');
      void loadViewPrefs();
    });
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

  // ---- item 写操作 ----
  async function updateItem(id: string, patch: Parameters<typeof api.itemUpdate>[1], path?: string) {
    try {
      const updated = await api.itemUpdate(id, patch, path);
      applyUpdatedItem(updated, true);
    } catch (e) {
      showToast(errorText(e));
    }
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

  /** 选中项逐个位置移入回收站（每张卡片即一个位置，删除只动该位置，其余位置保留） */
  async function trashSelected() {
    const keys = [...selection.value];
    for (const key of keys) {
      const { id, path } = splitKey(key);
      try {
        await api.itemDelete(id, path);
      } catch (e) {
        showToast(errorText(e));
      }
    }
    clearSelection();
    debouncedSkeletonReload(() => void reloadSkeleton());
  }

  /** 删除单个文件位置（Inspector 文件位置列表）：item 其余位置保留；
 *  删除后经 SSE item.updated 就地刷新，最后一个库内位置被删时按整项回收 */
  async function deleteLocation(id: string, path: string) {
    try {
      await api.itemDelete(id, path);
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function restoreSelected() {
    const keys = [...selection.value];
    let failed = 0;
    for (const key of keys) {
      const { id, path } = splitKey(key);
      try {
        await api.itemRestore(id, path);
      } catch (e) {
        failed++;
        showToast(errorText(e));
      }
    }
    clearSelection();
    if (failed === 0) {
      showToast('已恢复');
    }
    debouncedSkeletonReload(() => void reloadSkeleton());
  }

  async function clearTrash() {
    try {
      await api.trashClear();
      showToast('回收站已清空');
      if (isTrash.value) {
        debouncedSkeletonReload(() => void reloadSkeleton());
      }
    } catch (e) {
      showToast(errorText(e));
    }
  }

  /** 改库显示名（当前库）：写库内 config.toml 的 name；成功后就地更新库信息并返回 true */
  async function renameLibrary(name: string): Promise<boolean> {
    try {
      library.value = await api.libraryRename(name);
      return true;
    } catch (e) {
      showToast(errorText(e));
      return false;
    }
  }

  /** 手动「刷新缓存」：强制遍历全部文件做复用判定（不读文件内容），收敛监听漏事件与直接改目录 */
  async function refreshLibrary() {
    try {
      await api.rescan();
      showToast('正在刷新缓存…');
    } catch (e) {
      showToast(errorText(e));
    }
  }

  /** 按范围刷新派生缓存（补缺失模式）：修复 0 × 0 宽高、缺失缩略图/调色板；修复项经 item.updated 自动刷新。
   *  附带消失对账：源文件已删但索引残留的失效位置会被清除（watcher 漏事件时的手动收敛入口） */
  async function refreshCache(type: 'folder' | 'category' | 'tag' | 'library', value?: string, label?: string) {
    try {
      const res = await api.refreshCache(type, value);
      const parts: string[] = [];
      if (res.removed > 0) {
        parts.push(`已清除 ${res.removed} 个失效位置`);
      }
      parts.push(res.dispatched > 0 ? `正在刷新「${label ?? type}」缓存（${res.dispatched} 项）` : `「${label ?? type}」派生缓存完好，无需修复`);
      showToast(parts.join('，'));
      if (res.removed > 0) {
        debouncedSkeletonReload(() => void reloadSkeleton());
      }
    } catch (e) {
      showToast(errorText(e));
    }
  }

  /** 为全部选中项追加分类(内容级：同 hash 多位置只应用一次)。
   *  已有该分类的 id 从已加载详情一次构建（未加载的由服务端空操作跳过，不为过滤拉全量详情） */
  async function addCategoryToSelected(name: string) {
    const existing = new Set(
      [...details.value.values()].filter((i) => i.categories.includes(name)).map((i) => i.id),
    );
    const ids = selectionUniqueIds().filter((id) => !existing.has(id));
    await batchUpdate(ids, { add_categories: [name] }, '已添加分类');
    taxonomyHooks?.refreshTaxonomy();
  }

  /** 为全部选中项追加标签(同 addCategoryToSelected 的过滤策略) */
  async function addTagToSelected(tag: string) {
    const existing = new Set(
      [...details.value.values()].filter((i) => i.tags.includes(tag)).map((i) => i.id),
    );
    const ids = selectionUniqueIds().filter((id) => !existing.has(id));
    await batchUpdate(ids, { add_tags: [tag] }, '已添加标签');
    taxonomyHooks?.refreshTaxonomy();
  }

  /** 选中集的内容 id（去重）：元数据类批量操作（标签/分类/评分）按内容应用一次 */
  function selectionUniqueIds(): string[] {
    return [...new Set(selection.value.map((key) => splitKey(key).id))];
  }

  /** 将全部选中项移动到目标文件夹(位置级：每位置各移;空字符串为根目录);已在目标文件夹的位置跳过;完成后立即刷新文件夹树 */
  async function moveSelectedToFolder(path: string) {
    const targets = selection.value
      .map((key) => splitKey(key))
      .filter(({ path: p }) => {
        const dir = p.includes('/') ? p.slice(0, p.lastIndexOf('/')) : '';
        return dir !== path;
      });
    await batchUpdate(
      targets.map((t) => t.id),
      { paths: targets.map((t) => t.path), folder_path: path },
      '已移动',
    );
    taxonomyHooks?.refreshFolders();
  }

  /** 批量设置选中项评分(内容级去重;多选面板与右键菜单共用) */
  async function setStarForSelected(star: number) {
    await batchUpdate(selectionUniqueIds(), { star }, '已设置评分');
  }

  /** 批量端点统一入口:missing(内容不存在/移动冲突)在结果中提示,不整体失败 */
  async function batchUpdate(ids: string[], patch: Parameters<typeof api.itemBatchUpdate>[1], doneText: string) {
    if (ids.length === 0) {
      return;
    }
    try {
      const res = await api.itemBatchUpdate(ids, patch);
      const skipped = res.missing_ids.length;
      showToast(skipped > 0 ? `${doneText}(${skipped} 个未处理)` : doneText);
    } catch (e) {
      showToast(errorText(e));
    }
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
          indexProgress.value = p.pending + p.active > 0
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
    view, query, skeleton, details, total, totalSize, viewTitle, loading, windowLoading, selection, selectionSet, skeletonSizeMap, library, thumbSize, setUserThumbSize, searchText, toast, deleteLocation, taskBacklog, indexProgress, sidebarVisible, filterBarVisible, viewerMode, viewPrefs,
    isTrash, canGoBack, canGoForward, currentFolderPath, selectedItems, primarySelected, hasActiveFilters,
    init, setView, correctView, goBack, goForward, toggleSidebar, toggleFilterBar, setQuery, resetSort, submitSearch, resetList, ensureWindow, reloadSkeleton,
    select, selectAll, clearSelection,
    updateItem, trashSelected, restoreSelected, clearTrash, refreshLibrary, refreshCache, renameLibrary,
    addCategoryToSelected, addTagToSelected, moveSelectedToFolder, setStarForSelected,
    showToast, applyEvent, setGlobalFilter,
  };
});

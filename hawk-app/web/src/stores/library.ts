// Pinia 主 store：视图/查询/列表/选择集/文件夹树，以及全部业务 action。
// 组件不直接调 api，一切经 action；SSE 事件经 applyEvent 分发。
import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '../api/endpoints';
import { ApiError } from '../api/client';
import { blobToBase64, rotateImage, type RotateAngle } from '../imageEdit';
import type { CategoryInfo, FolderNode, Item, ItemListRequest, LibraryInfo, QueryState, SkeletonItem, TagInfo, ViewPrefs, ViewState } from '../types';

/** 首屏窗口大小（条目数）：覆盖首屏 + 少量预取；之后按视口区间补数据 */
const INITIAL_WINDOW = 150;

/** 全局默认排序（无任何记忆时的回落） */
const DEFAULT_SORT: Pick<QueryState, 'orderBy' | 'order'> = { orderBy: 'modification_time', order: 'desc' };

const ERROR_TEXT: Record<string, string> = {
  FILE_EXISTS: '同名文件或文件夹已存在',
  ITEM_NOT_FOUND: '素材不存在或已被移除',
  FOLDER_NOT_FOUND: '文件夹不存在',
  CATEGORY_NOT_FOUND: '分类不存在',
  CATEGORY_EXISTS: '分类已存在',
  TAG_NOT_FOUND: '标签不存在',
  UNSUPPORTED_FORMAT: '不支持的格式',
  INVALID_PARAM: '参数无效',
  NETWORK: '无法连接 hawk-server',
};

function errorText(e: unknown): string {
  return e instanceof ApiError ? (ERROR_TEXT[e.code] ?? e.message) : String(e);
}

/** 简易防抖（模块级，store 单例无需多实例隔离） */
function debounce(ms: number) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return (fn: () => void) => {
    clearTimeout(timer);
    timer = setTimeout(fn, ms);
  };
}

export const useLibraryStore = defineStore('library', () => {
  // ---- state ----
  const view = ref<ViewState>({ kind: 'all' });
  const query = ref<QueryState>({ keywords: [], orderBy: 'modification_time', order: 'desc' });
  /** 当前视图全量骨架（id/width/height/star，与 item/list 同查询同排序）：布局与滚动条总高的唯一依据 */
  const skeleton = ref<SkeletonItem[]>([]);
  /** 已拉取的详情（视口窗口 + 预取），按 id 索引；不在视口的行只留骨架占位不渲染 */
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
  const folders = ref<FolderNode | null>(null);
  const categories = ref<CategoryInfo[]>([]);
  const tagList = ref<TagInfo[]>([]);
  const trashTotal = ref(0);
  // 侧栏智能条目计数（limit:1 只取 total）
  const rootCount = ref(0);
  const uncategorizedCount = ref(0);
  const untaggedCount = ref(0);
  const library = ref<LibraryInfo | null>(null);
  const thumbSize = ref(160);
  /** 搜索框草稿（顶栏与检查器顶搜索框共用一份，回车提交为 keywords） */
  const searchText = ref('');
  const previewId = ref<string | null>(null);
  const toast = ref<string | null>(null);
  /** 导入进度：null 无任务；total=0 表示收集文件阶段（不定态），done 为已处理数 */
  const importProgress = ref<{ total: number; done: number } | null>(null);
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
  /** 预览浮层 sticky item：详情未加载时不置空（浮层不卸载，滑动切换动画与状态不丢）；关闭时随 previewId 归零 */
  let lastPreviewItem: Item | null = null;
  const previewItem = computed(() => {
    const current = previewId.value ? (details.value.get(previewId.value) ?? null) : null;
    if (current) {
      lastPreviewItem = current;
    }
    // sticky:详情未加载时不置空——避免浮层卸载重建导致滑动切换动画与状态丢失；关闭时随 previewId 归零
    return current ?? (previewId.value ? lastPreviewItem : null);
  });
  /** 当前视图条目数（= 骨架长度；骨架未加载时为 0） */
  const total = computed(() => skeleton.value.length);
  const previewIndex = computed(() => skeleton.value.findIndex((i) => i.id === previewId.value));
  /** 缩略图尺寸候选（library/info 的 thumbnail_sizes 升序；缺字段兜底默认档），网格 img srcset 用 */
  const thumbSizes = computed<number[]>(() =>
    (library.value?.thumbnail_sizes ?? [256, 1024]).map((s) => Number(s)).sort((a, b) => a - b),
  );
  const previewNavId = (step: 1 | -1) => {
    const next = previewIndex.value >= 0 ? skeleton.value[previewIndex.value + step] : undefined;
    return next?.id ?? null;
  };

  /** 扁平化的文件夹树（移动到文件夹等选择控件用），含根目录 */
  const flatFolders = computed(() => {
    const list: { path: string; label: string }[] = [{ path: '', label: '（根目录）' }];
    const walk = (node: FolderNode, depth: number) => {
      for (const child of node.children) {
        list.push({ path: child.path, label: '　'.repeat(depth) + child.name });
        walk(child, depth + 1);
      }
    };
    if (folders.value) {
      walk(folders.value, 0);
    }
    return list;
  });

  /** 扁平化的分类列表（添加到分类选择控件用） */
  const categoryOptions = computed(() => categories.value.map((c) => ({ name: c.name })));

  // ---- 内部 ----
  const debouncedSkeletonReload = debounce(200);
  const debouncedRefreshFolders = debounce(300);
  const debouncedRefreshTaxonomy = debounce(300);
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
      folders: view.value.kind === 'folder' ? [view.value.path] : view.value.kind === 'root' ? [''] : undefined,
      folders_exact: view.value.kind === 'root' ? true : undefined,
      without_categories: view.value.kind === 'uncategorized' ? true : undefined,
      without_tags: view.value.kind === 'untagged' ? true : undefined,
      categories: view.value.kind === 'category' ? [view.value.name] : undefined,
      tags: view.value.kind === 'tag' ? [view.value.name] : undefined,
    };
  }

  function showToast(message: string) {
    toast.value = message;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast.value = null), 3000);
  }

  // ---- 初始与查询 ----
  /** 局域网 web 查看（viewer token）：只读模式，全部写入口隐藏；服务端 403 为最终防线 */
  const viewerMode = ref(false);

  async function init() {
    const info = await api.appInfo();
    viewerMode.value = info.access === 'viewer';
    library.value = await api.libraryInfo();

    // 换库/应用设置重启复用本入口：清掉上一库的会话状态，避免视图/预览/进度指示残留。
    // 视图回退由 restoreView 负责（无记忆时回默认视图），这里只清与库无关的记忆
    query.value = { keywords: [], orderBy: 'modification_time', order: 'desc' };
    searchText.value = '';
    clearSelection();
    closePreview();
    closeEditor();
    taskBacklog.value = null;
    indexProgress.value = null;

    await Promise.all([refreshFolders(), refreshTaxonomy(), loadViewPrefs()]);
    restoreView();
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

  /** 视图记忆：按素材库路径存 localStorage（同一台机器多库互不干扰） */
  function viewStorageKey() {
    return `hawk:lastView:${library.value?.path ?? ''}`;
  }

  function restoreView() {
    // 恢复不了（无记忆/目标已删/数据损坏）一律回退全部素材：
    // 换库复用 init 时 view 残留上一库取值，任何 return 路径都必须显式重置
    const fallback: ViewState = { kind: 'all' };
    try {
      const saved = localStorage.getItem(viewStorageKey());
      if (!saved) {
        view.value = fallback;
        return;
      }
      const parsed = JSON.parse(saved) as ViewState;
      const valid =
        (parsed.kind !== 'folder' || folderExists(parsed.path)) &&
        (parsed.kind !== 'category' || categoryExists(parsed.name)) &&
        (parsed.kind !== 'tag' || tagList.value.some((t) => t.name === parsed.name));
      view.value = valid ? parsed : fallback;
    } catch {
      view.value = fallback; // 损坏的持久化数据
    }
  }

  function folderExists(path: string): boolean {
    const walk = (node: FolderNode): boolean => node.path === path || node.children.some(walk);
    return folders.value ? walk(folders.value) : false;
  }

  function categoryExists(name: string): boolean {
    return categories.value.some((c) => c.name === name);
  }

  /** 应用视图：持久化 + 应用记忆排序 + 清选择 + 重查列表（setView/goBack/correctView 的公共收尾） */
  function applyView(v: ViewState) {
    view.value = v;
    localStorage.setItem(viewStorageKey(), JSON.stringify(v));
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

  /**
   * 解析视图的有效排序：folder 自底向上沿父链继承（子文件夹自己的设置优先），
   * category/tag 无层级直接回落默认；无记忆语义的视图用全局默认
   */
  function resolveSort(v: ViewState): Pick<QueryState, 'orderBy' | 'order'> {
    const hit = (scope: string) => {
      const e = viewPrefs.value[scope];
      return e ? { orderBy: e.order_by, order: e.order } : undefined;
    };

    if (v.kind === 'category' || v.kind === 'tag') {
      return hit(`${v.kind}:${v.name}`) ?? DEFAULT_SORT;
    }

    if (v.kind === 'folder') {
      for (let dir = v.path; ; dir = dir.includes('/') ? dir.slice(0, dir.lastIndexOf('/')) : '') {
        const h = hit(`folder:${dir}`);
        if (h) return h;
        if (dir === '') break;
      }
    }

    return DEFAULT_SORT;
  }

  /** 应用视图的有效排序（applyView/初始化/恢复默认用；只改排序字段，不动筛选条件；不触发持久化） */
  function applySortForView(v: ViewState) {
    const sort = resolveSort(v);
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
      if (!details.value.has(sk[i].id)) {
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
        map.set(item.id, item);
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
      const ids = new Set(res.items.map((i) => i.id));
      selection.value = selection.value.filter((id) => ids.has(id));
      if (details.value.size > 0) {
        const map = new Map(details.value);
        let changed = false;
        for (const id of [...map.keys()]) {
          if (!ids.has(id)) {
            map.delete(id);
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

  // ---- 选择 ----
  function select(id: string, mod?: 'range' | 'toggle') {
    if (mod === 'range' && selection.value.length > 0) {
      const anchor = selection.value.at(-1)!;
      const a = skeleton.value.findIndex((i) => i.id === anchor);
      const b = skeleton.value.findIndex((i) => i.id === id);
      if (a >= 0 && b >= 0) {
        const [from, to] = a < b ? [a, b] : [b, a];
        selection.value = skeleton.value.slice(from, to + 1).map((i) => i.id);
        return;
      }
    }
    if (mod === 'toggle') {
      selection.value = selection.value.includes(id)
        ? selection.value.filter((s) => s !== id)
        : [...selection.value, id];
      return;
    }
    selection.value = [id];
  }

  function selectAll() {
    // 基于骨架（全量），不是仅视口窗口
    selection.value = skeleton.value.map((i) => i.id);
  }

  function clearSelection() {
    selection.value = [];
  }

  // ---- item 写操作 ----
  async function updateItem(id: string, patch: Parameters<typeof api.itemUpdate>[1], path?: string) {
    try {
      const updated = await api.itemUpdate(id, patch, path);
      applyUpdatedItem(updated);
    } catch (e) {
      showToast(errorText(e));
    }
  }

  /** 无过滤的「全部素材」视图：item.updated 不可能改变成员资格（进出回收站有独立事件），可原地更新 */
  function isUnfilteredView() {
    return (
      view.value.kind === 'all' &&
      query.value.keywords.length === 0 &&
      query.value.star === undefined &&
      !query.value.color
    );
  }

  /**
   * item.updated 的统一入口（updateItem 响应与 SSE 共用）。
   * 详情在缓存中就地替换立即反映；骨架上的 star 同步（★ 角标）。
   * 过滤视图（文件夹/分类/标签/回收站）或激活查询条件时防抖重载骨架——
   * 成员资格可能已变化（移出当前文件夹、摘掉当前分类/标签等），成员判定以服务端查询为准。
   * 标签/分类集合真的变化时才刷分类计数：缩略图就绪等高频 updated 与计数无关，不再每事件刷 taxonomy。
   */
  function applyUpdatedItem(updated: Item) {
    const prev = details.value.get(updated.id);
    const taxonomyChanged =
      prev !== undefined &&
      (!sameNameSet(prev.tags, updated.tags) || !sameNameSet(prev.categories, updated.categories));
    if (details.value.has(updated.id)) {
      const map = new Map(details.value);
      map.set(updated.id, updated);
      details.value = map;
    }
    const skIdx = skeleton.value.findIndex((s) => s.id === updated.id);
    if (skIdx >= 0 && skeleton.value[skIdx].star !== updated.star) {
      const next = skeleton.value.slice();
      next[skIdx] = { ...next[skIdx], star: updated.star };
      skeleton.value = next;
    }
    if (!isUnfilteredView()) {
      debouncedSkeletonReload(() => void reloadSkeleton());
    }
    if (taxonomyChanged) {
      debouncedRefreshTaxonomy(() => void refreshTaxonomy());
    }
  }

  /** 名称集合相等比较（标签/分类为无序去重列表） */
  function sameNameSet(a: string[] | undefined, b: string[] | undefined): boolean {
    const x = a ?? [];
    const y = b ?? [];
    return x.length === y.length && x.every((v) => y.includes(v));
  }

  async function trashSelected() {
    const ids = [...selection.value];
    for (const id of ids) {
      try {
        await api.itemDelete(id);
      } catch (e) {
        showToast(errorText(e));
      }
    }
    clearSelection();
    debouncedSkeletonReload(() => void reloadSkeleton());
  }

  async function restoreSelected() {
    const ids = [...selection.value];
    let failed = 0;
    for (const id of ids) {
      try {
        await api.itemRestore(id);
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

  /** 手动「刷新缓存」：强制遍历全部文件做复用判定（不读文件内容），收敛监听漏事件与直接改目录 */
  async function refreshLibrary() {
    try {
      await api.rescan();
      showToast('正在刷新缓存…');
    } catch (e) {
      showToast(errorText(e));
    }
  }

  /** 导入开始：拖拽落下即调用，覆盖「收集文件」阶段；已有任务时拒绝并提示 */
  function importBegin(): boolean {
    if (importProgress.value) {
      showToast('已有导入任务进行中');
      return false;
    }
    importProgress.value = { total: 0, done: 0 };
    return true;
  }

  /** 拖拽导入：逐个 itemAddByPath（server 逐文件完成复制/哈希/索引/缩略图后才返回），done 逐项推进 */
  async function importPaths(paths: string[]) {
    if (paths.length === 0) {
      importProgress.value = null;
      showToast('未找到可导入的文件');
      return;
    }
    importProgress.value = { total: paths.length, done: 0 };
    let added = 0;
    let existed = 0;
    let failed = 0;
    for (const path of paths) {
      try {
        const res = await api.itemAddByPath(path, { folder_path: currentFolderPath.value ?? undefined });
        res.already_existed ? existed++ : added++;
      } catch {
        failed++;
      }
      importProgress.value.done += 1;
    }
    importProgress.value = null;
    showToast(`导入完成：新增 ${added}${existed ? `，已存在 ${existed}` : ''}${failed ? `，失败 ${failed}` : ''}`);
    // SSE item.added 已触发防抖骨架重载，这里不重复拉取
  }

  // ---- 文件夹写操作 ----
  async function folderCreate(parentPath: string, name: string) {
    try {
      await api.folderCreate(name, parentPath || undefined);
      await refreshFolders();
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function folderRename(path: string, name: string) {
    try {
      await api.folderUpdate(path, { name });
      await refreshFolders();
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function folderDelete(path: string) {
    try {
      await api.folderDelete(path);
      await refreshFolders();
      if (currentFolderPath.value === path || currentFolderPath.value?.startsWith(path + '/')) {
        correctView({ kind: 'all' });
      }
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function refreshFolders() {
    try {
      folders.value = await api.folderList();
    } catch (e) {
      showToast(errorText(e));
    }
  }

  // ---- 分类/标签 ----

  async function refreshTaxonomy() {
    try {
      const [categoryList, tags, trash, root, uncategorized, untagged] = await Promise.all([
        api.categoryList(),
        api.tagList(),
        api.itemList({ in_trash: true, limit: 1 }),
        api.itemList({ folders: [''], folders_exact: true, limit: 1 }),
        api.itemList({ without_categories: true, limit: 1 }),
        api.itemList({ without_tags: true, limit: 1 }),
      ]);
      categories.value = categoryList;
      tagList.value = tags;
      trashTotal.value = Number(trash.total);
      rootCount.value = Number(root.total);
      uncategorizedCount.value = Number(uncategorized.total);
      untaggedCount.value = Number(untagged.total);
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function categoryCreate(name: string) {
    try {
      await api.categoryCreate(name);
      await refreshTaxonomy();
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function categoryRename(name: string, newName: string) {
    try {
      await api.categoryUpdate(name, newName);
      await refreshTaxonomy();
      // 当前视图正在该分类下 → 跟随新名字
      if (view.value.kind === 'category' && view.value.name === name) {
        correctView({ kind: 'category', name: newName });
      }
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function categoryDelete(name: string) {
    try {
      await api.categoryDelete(name);
      await refreshTaxonomy();
      if (view.value.kind === 'category' && view.value.name === name) {
        correctView({ kind: 'all' });
      }
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function tagCreate(name: string) {
    try {
      await api.tagCreate(name);
      await refreshTaxonomy();
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function tagRename(name: string, newName: string) {
    try {
      await api.tagUpdate(name, newName);
      await refreshTaxonomy();
      if (view.value.kind === 'tag' && view.value.name === name) {
        correctView({ kind: 'tag', name: newName });
      }
    } catch (e) {
      showToast(errorText(e));
    }
  }

  async function tagDelete(name: string) {
    try {
      await api.tagDelete(name);
      await refreshTaxonomy();
      if (view.value.kind === 'tag' && view.value.name === name) {
        correctView({ kind: 'all' });
      }
    } catch (e) {
      showToast(errorText(e));
    }
  }

  /** 选中项详情补齐：加标签/分类需读现有值合并，批量选中（含视口外）时先按需拉取 */
  async function ensureSelectionDetails() {
    const missing = selection.value.filter((id) => !details.value.has(id));
    if (missing.length === 0) {
      return;
    }
    try {
      const res = await api.itemList({ ...listParams(), ids: missing, offset: 0, limit: missing.length });
      const map = new Map(details.value);
      for (const item of res.items) {
        map.set(item.id, item);
      }
      details.value = map;
    } catch {
      // 尽力而为：拉不到的选中项在后续合并中跳过
    }
  }

  /** 为全部选中项追加分类(去重,保留已有);完成后立即刷新分类计数,不等 SSE 防抖 */
  async function addCategoryToSelected(name: string) {
    await ensureSelectionDetails();
    const ids = selection.value.filter((id) => !(details.value.get(id)?.categories ?? []).includes(name));
    await batchUpdate(ids, { add_categories: [name] }, '已添加分类');
    void refreshTaxonomy();
  }

  /** 为全部选中项追加标签(去重,保留已有);完成后立即刷新标签计数,不等 SSE 防抖 */
  async function addTagToSelected(tag: string) {
    await ensureSelectionDetails();
    const ids = selection.value.filter((id) => !(details.value.get(id)?.tags ?? []).includes(tag));
    await batchUpdate(ids, { add_tags: [tag] }, '已添加标签');
    void refreshTaxonomy();
  }

  /** 将全部选中项移动到目标文件夹(空字符串为根目录);已在目标文件夹的项跳过;完成后立即刷新文件夹树 */
  async function moveSelectedToFolder(path: string) {
    await ensureSelectionDetails();
    const ids = selection.value.filter((id) => (details.value.get(id)?.folders?.[0] ?? '') !== path);
    await batchUpdate(ids, { folder_path: path }, '已移动');
    void refreshFolders();
  }

  /** 批量设置选中项评分(多选面板与右键菜单共用) */
  async function setStarForSelected(star: number) {
    await batchUpdate([...selection.value], { star }, '已设置评分');
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

  // ---- 预览浮层 ----
  function openPreview(id: string) {
    previewId.value = id;
    // 详情可能未加载（如键盘导航跳到视口外项）：按骨架索引补拉，到位后浮层即出现
    const idx = skeleton.value.findIndex((s) => s.id === id);
    if (idx >= 0) {
      void ensureWindow(idx, idx + 1);
    }
  }

  function closePreview() {
    previewId.value = null;
  }

  function navigatePreview(step: 1 | -1) {
    const next = previewNavId(step);
    if (next) {
      openPreview(next);
    }
  }

  /** 图片编辑窗口的目标 item(全局单例):网格/预览浮层右键「编辑图片…」均可打开 */
  const editorTarget = ref<Item | null>(null);

  function openEditor(item: Item) {
    editorTarget.value = item;
  }

  function closeEditor() {
    editorTarget.value = null;
  }

  /**
   * 编辑窗口保存:解码/旋转/重编码在客户端完成(编辑计算归客户端),经 item/replace 提交存储层。
   * 内容哈希变化导致 id 漂移:新 item 就地替换详情;预览若正打开该 item 则跟随新 id;
   * 骨架/选择的旧 id 由 SSE item.removed 清理。返回是否成功,调用方据此关闭编辑窗口。
   */
  async function saveImageEdit(id: string, angle: RotateAngle): Promise<boolean> {
    const item = details.value.get(id);
    if (!item) {
      return false;
    }
    try {
      // no-store:item/file 带 Cache-Control immutable,<img> 加载会把无 ACAO 的响应存进磁盘缓存,
      // 默认 cache 模式的 fetch 复用该缓存条目会被 CORS 拒绝(浏览器对 <img> 请求不携 Origin,服务端不返回 ACAO)
      const res = await fetch(api.fileUrl(item.id), { cache: 'no-store' });
      if (!res.ok) {
        throw new Error('原图获取失败');
      }
      const rotated = await rotateImage(await res.blob(), angle, item.ext);
      const updated = await api.itemReplace(item.id, await blobToBase64(rotated));
      const map = new Map(details.value);
      map.delete(item.id);
      map.set(updated.id, updated);
      details.value = map;
      if (previewId.value === item.id) {
        previewId.value = updated.id;
      }
      showToast('已保存');
      return true;
    } catch (e) {
      // ApiError 走错误码翻译(如 UNSUPPORTED_FORMAT),本地 Error 直接取 message
      showToast(e instanceof ApiError ? errorText(e) : e instanceof Error ? e.message : String(e));
      return false;
    }
  }

  // ---- SSE ----
  // 事件与副作用的对应关系（不无条件全刷，后台事件爆发期不制造请求风暴）：
  // - item.updated：详情/骨架就地更新；标签/分类集合变化才刷分类计数
  // - item.added/restored/trashed/removed：成员与计数以服务端查询为准 → 防抖重载骨架 + 刷分类计数
  // - folder.changed：只刷文件夹树（目录结构变化的唯一信号）
  // - task.progress：只更新对应的后台任务指示
  function applyEvent(type: string, payload: unknown) {
    switch (type) {
      case 'item.updated': {
        applyUpdatedItem(payload as Item);
        break;
      }
      case 'item.added':
      case 'item.restored':
        // 新 item 的落点（成员/次序）只能以服务端查询为准：防抖重载骨架，视口窗口随后按需补齐
        debouncedSkeletonReload(() => void reloadSkeleton());
        debouncedRefreshTaxonomy(() => void refreshTaxonomy());
        break;
      case 'item.trashed':
      case 'item.removed': {
        const id = (payload as { id: string }).id;
        // 就地移除立即反馈；回收站视图同事件意味着「进来了」，统一以防抖重载兜底
        if (details.value.has(id)) {
          const map = new Map(details.value);
          map.delete(id);
          details.value = map;
        }
        if (skeleton.value.some((s) => s.id === id)) {
          skeleton.value = skeleton.value.filter((s) => s.id !== id);
        }
        selection.value = selection.value.filter((s) => s !== id);
        debouncedSkeletonReload(() => void reloadSkeleton());
        debouncedRefreshTaxonomy(() => void refreshTaxonomy());
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
        // 目录结构变化(本端操作/外部进程/对账兜底):重拉文件夹树;与骨架成员和分类/标签计数无关
        debouncedRefreshFolders(() => void refreshFolders());
        break;
    }
  }

  return {
    view, query, skeleton, details, total, totalSize, viewTitle, loading, windowLoading, selection, folders, categories, tagList, trashTotal, rootCount, uncategorizedCount, untaggedCount, library, thumbSize, searchText, previewId, toast, importProgress, taskBacklog, indexProgress, sidebarVisible, filterBarVisible, editorTarget, viewerMode, viewPrefs,
    isTrash, canGoBack, canGoForward, currentFolderPath, selectedItems, primarySelected, previewItem, previewIndex, previewNavId, flatFolders, categoryOptions, thumbSizes, hasActiveFilters,
    init, setView, goBack, goForward, toggleSidebar, toggleFilterBar, setQuery, resetSort, submitSearch, resetList, ensureWindow, reloadSkeleton,
    select, selectAll, clearSelection,
    updateItem, trashSelected, restoreSelected, clearTrash, refreshLibrary, importBegin, importPaths,
    folderCreate, folderRename, folderDelete, refreshFolders,
    refreshTaxonomy, categoryCreate, categoryRename, categoryDelete, tagCreate, tagRename, tagDelete, addCategoryToSelected, addTagToSelected, moveSelectedToFolder, setStarForSelected,
    openPreview, closePreview, navigatePreview, saveImageEdit, openEditor, closeEditor, showToast, applyEvent,
  };
});

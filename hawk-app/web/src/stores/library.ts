// Pinia 主 store：视图/查询/列表/选择集/文件夹树，以及全部业务 action。
// 组件不直接调 api，一切经 action；SSE 事件经 applyEvent 分发。
import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '../api/endpoints';
import { ApiError } from '../api/client';
import type { CategoryInfo, FolderNode, Item, ItemListRequest, LibraryInfo, QueryState, TagInfo, ViewState } from '../types';

const PAGE_SIZE = 100;

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
  const items = ref<Item[]>([]);
  const total = ref(0);
  /** 当前视图（含筛选）未分页的全量字节数合计，检查器「分区状态」用 */
  const totalSize = ref(0);
  const loading = ref(false);
  const endReached = ref(false);
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
  const previewId = ref<string | null>(null);
  const toast = ref<string | null>(null);
  /** 导入进度：null 无任务；total=0 表示收集文件阶段（不定态），done 为已处理数 */
  const importProgress = ref<{ total: number; done: number } | null>(null);
  const sidebarVisible = ref(true);
  /** 浏览历史（会话内）：setView 压入，前进/后退在栈内移动 */
  const viewHistory = ref<ViewState[]>([]);
  const historyIndex = ref(-1);

  // ---- getters ----
  const isTrash = computed(() => view.value.kind === 'trash');
  const canGoBack = computed(() => historyIndex.value > 0);
  const canGoForward = computed(() => historyIndex.value >= 0 && historyIndex.value < viewHistory.value.length - 1);
  const currentFolderPath = computed(() => (view.value.kind === 'folder' ? view.value.path : null));
  const selectedItems = computed(
    () => selection.value.map((id) => items.value.find((i) => i.id === id)).filter((i): i is Item => !!i),
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
  const previewItem = computed(() => items.value.find((i) => i.id === previewId.value) ?? null);
  /** 缩略图尺寸候选（library/info 的 thumbnail_sizes 升序；缺字段兜底默认档），网格 img srcset 用 */
  const thumbSizes = computed<number[]>(() =>
    (library.value?.thumbnail_sizes ?? [256, 1024]).map((s) => Number(s)).sort((a, b) => a - b),
  );
  const previewNavId = (step: 1 | -1) => {
    const idx = items.value.findIndex((i) => i.id === previewId.value);
    return items.value[idx + step]?.id ?? null;
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
  const debouncedRefresh = debounce(200);
  const debouncedRefreshFolders = debounce(300);
  const debouncedRefreshTaxonomy = debounce(300);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  function buildListParams(offset: number, limit: number): ItemListRequest {
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
      offset,
      limit,
    };
  }

  function showToast(message: string) {
    toast.value = message;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast.value = null), 3000);
  }

  // ---- 初始与查询 ----
  async function init() {
    library.value = await api.libraryInfo();
    await Promise.all([refreshFolders(), refreshTaxonomy()]);
    restoreView();
    // 历史栈以恢复后的视图为起点
    viewHistory.value = [view.value];
    historyIndex.value = 0;
    await resetList();
  }

  /** 视图记忆：按素材库路径存 localStorage（同一台机器多库互不干扰） */
  function viewStorageKey() {
    return `hawk:lastView:${library.value?.path ?? ''}`;
  }

  function restoreView() {
    try {
      const saved = localStorage.getItem(viewStorageKey());
      if (!saved) {
        return;
      }
      const parsed = JSON.parse(saved) as ViewState;
      if (parsed.kind === 'folder' && !folderExists(parsed.path)) {
        return; // 上次浏览的文件夹已被删除 → 回退全部素材
      }
      if (parsed.kind === 'category' && !categoryExists(parsed.name)) {
        return;
      }
      if (parsed.kind === 'tag' && !tagList.value.some((t) => t.name === parsed.name)) {
        return;
      }
      view.value = parsed;
    } catch {
      // 损坏的持久化数据忽略
    }
  }

  function folderExists(path: string): boolean {
    const walk = (node: FolderNode): boolean => node.path === path || node.children.some(walk);
    return folders.value ? walk(folders.value) : false;
  }

  function categoryExists(name: string): boolean {
    return categories.value.some((c) => c.name === name);
  }

  /** 应用视图：持久化 + 清选择 + 重查列表（setView/goBack/correctView 的公共收尾） */
  function applyView(v: ViewState) {
    view.value = v;
    localStorage.setItem(viewStorageKey(), JSON.stringify(v));
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

  function setQuery(patch: Partial<QueryState>) {
    Object.assign(query.value, patch);
    void resetList();
  }

  async function resetList() {
    items.value = [];
    endReached.value = false;
    total.value = 0;
    totalSize.value = 0;
    await fetchMore();
  }

  async function fetchMore() {
    if (loading.value || endReached.value) {
      return;
    }
    loading.value = true;
    try {
      const res = await api.itemList(buildListParams(items.value.length, PAGE_SIZE));
      items.value = [...items.value, ...res.items];
      total.value = Number(res.total);
      totalSize.value = Number(res.total_size);
      endReached.value = items.value.length >= total.value;
    } catch (e) {
      showToast(errorText(e));
    } finally {
      loading.value = false;
    }
  }

  /** SSE 驱动的刷新：重查已加载范围并整体替换，尽量保持滚动位置 */
  async function refresh() {
    const limit = Math.max(items.value.length, PAGE_SIZE);
    try {
      const res = await api.itemList(buildListParams(0, limit));
      items.value = res.items;
      total.value = Number(res.total);
      totalSize.value = Number(res.total_size);
      endReached.value = items.value.length >= total.value;
      // 保持不变式「选择 ⊆ 列表」：不再属于当前视图的选中项一并摘除
      selection.value = selection.value.filter((id) => res.items.some((i) => i.id === id));
    } catch (e) {
      showToast(errorText(e));
    }
  }

  // ---- 选择 ----
  function select(id: string, mod?: 'range' | 'toggle') {
    if (mod === 'range' && selection.value.length > 0) {
      const anchor = selection.value.at(-1)!;
      const a = items.value.findIndex((i) => i.id === anchor);
      const b = items.value.findIndex((i) => i.id === id);
      if (a >= 0 && b >= 0) {
        const [from, to] = a < b ? [a, b] : [b, a];
        selection.value = items.value.slice(from, to + 1).map((i) => i.id);
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
    selection.value = items.value.map((i) => i.id);
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
   * 无过滤视图原地更新；过滤视图（文件夹/分类/标签/回收站）或激活查询条件时防抖整表刷新——
   * 成员资格可能已变化（移出当前文件夹、摘掉当前分类/标签等），成员判定以服务端查询为准。
   */
  function applyUpdatedItem(updated: Item) {
    if (isUnfilteredView()) {
      const idx = items.value.findIndex((i) => i.id === updated.id);
      if (idx >= 0) {
        items.value[idx] = updated;
      }
      return;
    }
    debouncedRefresh(() => void refresh());
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
    debouncedRefresh(() => void refresh());
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
    debouncedRefresh(() => void refresh());
  }

  async function clearTrash() {
    try {
      await api.trashClear();
      showToast('回收站已清空');
      if (isTrash.value) {
        await resetList();
      }
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
    debouncedRefresh(() => void refresh());
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

  /** 为全部选中项追加分类（去重，保留已有） */
  function addCategoryToSelected(name: string) {
    for (const id of selection.value) {
      const item = items.value.find((i) => i.id === id);
      if (item && !(item.categories ?? []).includes(name)) {
        void updateItem(id, { categories: [...(item.categories ?? []), name] });
      }
    }
  }

  /** 为全部选中项追加标签（去重，保留已有） */
  function addTagToSelected(tag: string) {
    for (const id of selection.value) {
      const item = items.value.find((i) => i.id === id);
      if (item && !(item.tags ?? []).includes(tag)) {
        void updateItem(id, { tags: [...(item.tags ?? []), tag] });
      }
    }
  }

  /** 将全部选中项移动到目标文件夹（空字符串为根目录） */
  function moveSelectedToFolder(path: string) {
    for (const id of selection.value) {
      void updateItem(id, { folder_path: path });
    }
  }

  // ---- 预览浮层 ----
  function openPreview(id: string) {
    previewId.value = id;
  }

  function closePreview() {
    previewId.value = null;
  }

  function navigatePreview(step: 1 | -1) {
    const next = previewNavId(step);
    if (next) {
      previewId.value = next;
    }
  }

  // ---- SSE ----
  function applyEvent(type: string, payload: unknown) {
    switch (type) {
      case 'item.updated': {
        applyUpdatedItem(payload as Item);
        break;
      }
      case 'item.added':
      case 'item.restored':
        debouncedRefresh(() => void refresh());
        break;
      case 'item.trashed':
      case 'item.removed': {
        const id = (payload as { id: string }).id;
        if (isTrash.value) {
          debouncedRefresh(() => void refresh());
        } else {
          items.value = items.value.filter((i) => i.id !== id);
          selection.value = selection.value.filter((s) => s !== id);
          total.value = Math.max(0, total.value - 1);
        }
        break;
      }
    }
    debouncedRefreshFolders(() => void refreshFolders());
    debouncedRefreshTaxonomy(() => void refreshTaxonomy());
  }

  return {
    view, query, items, total, totalSize, viewTitle, loading, endReached, selection, folders, categories, tagList, trashTotal, rootCount, uncategorizedCount, untaggedCount, library, thumbSize, previewId, toast, importProgress, sidebarVisible,
    isTrash, canGoBack, canGoForward, currentFolderPath, selectedItems, primarySelected, previewItem, previewNavId, flatFolders, categoryOptions, thumbSizes,
    init, setView, goBack, goForward, toggleSidebar, setQuery, resetList, fetchMore, refresh,
    select, selectAll, clearSelection,
    updateItem, trashSelected, restoreSelected, clearTrash, importBegin, importPaths,
    folderCreate, folderRename, folderDelete, refreshFolders,
    refreshTaxonomy, categoryCreate, categoryRename, categoryDelete, tagCreate, tagRename, tagDelete, addCategoryToSelected, addTagToSelected, moveSelectedToFolder,
    openPreview, closePreview, navigatePreview, showToast, applyEvent,
  };
});

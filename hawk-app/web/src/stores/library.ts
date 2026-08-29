// Pinia 主 store：视图/查询/列表/选择集/文件夹树，以及全部业务 action。
// 组件不直接调 api，一切经 action；SSE 事件经 applyEvent 分发。
import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '../api/endpoints';
import { ApiError } from '../api/client';
import type { CategoryInfo, FolderNode, Item, ItemListRequest, LibraryInfo, QueryState, SkeletonItem, TagInfo, ViewState } from '../types';

/** 首屏窗口大小（条目数）：覆盖首屏 + 少量预取；之后按视口区间补数据 */
const INITIAL_WINDOW = 150;

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
  const previewId = ref<string | null>(null);
  const toast = ref<string | null>(null);
  /** 导入进度：null 无任务；total=0 表示收集文件阶段（不定态），done 为已处理数 */
  const importProgress = ref<{ total: number; done: number } | null>(null);
  /** 缩略图后台积压（task.progress 事件驱动；null 表示无积压，进度条隐藏） */
  const taskBacklog = ref<{ pending: number; active: number } | null>(null);
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
  const previewItem = computed(() => (previewId.value ? (details.value.get(previewId.value) ?? null) : null));
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
   */
  async function reloadSkeleton() {
    const version = ++skeletonVersion;
    try {
      const res = await api.itemSkeleton(listParams());
      if (version !== skeletonVersion) {
        return;
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
   */
  function applyUpdatedItem(updated: Item) {
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

  /** 为全部选中项追加分类（去重，保留已有）；完成后立即刷新分类计数，不等 SSE 防抖 */
  async function addCategoryToSelected(name: string) {
    await ensureSelectionDetails();
    const jobs = [];
    for (const id of selection.value) {
      const item = details.value.get(id);
      if (item && !(item.categories ?? []).includes(name)) {
        jobs.push(updateItem(id, { categories: [...(item.categories ?? []), name] }));
      }
    }
    await Promise.all(jobs);
    void refreshTaxonomy();
  }

  /** 为全部选中项追加标签（去重，保留已有）；完成后立即刷新标签计数，不等 SSE 防抖 */
  async function addTagToSelected(tag: string) {
    await ensureSelectionDetails();
    const jobs = [];
    for (const id of selection.value) {
      const item = details.value.get(id);
      if (item && !(item.tags ?? []).includes(tag)) {
        jobs.push(updateItem(id, { tags: [...(item.tags ?? []), tag] }));
      }
    }
    await Promise.all(jobs);
    void refreshTaxonomy();
  }

  /** 将全部选中项移动到目标文件夹（空字符串为根目录）；已在目标文件夹的项跳过；完成后立即刷新文件夹树，不等 SSE 防抖 */
  async function moveSelectedToFolder(path: string) {
    await ensureSelectionDetails();
    const jobs = [];
    for (const id of selection.value) {
      const item = details.value.get(id);
      if (item && (item.folders?.[0] ?? '') === path) {
        continue;
      }
      jobs.push(updateItem(id, { folder_path: path }));
    }
    await Promise.all(jobs);
    void refreshFolders();
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

  // ---- SSE ----
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
        break;
      }
      case 'task.progress': {
        const p = payload as { task: string; pending: number; active: number };
        if (p.task !== 'thumbnail') {
          break;
        }
        // 积压归零撤掉指示；其余帧更新计数（节流由服务端 500ms 保证）
        taskBacklog.value = p.pending + p.active > 0 ? { pending: p.pending, active: p.active } : null;
        break;
      }
    }
    // task.progress 与文件夹树/分类无关，跳过下面的防抖刷新
    if (type === 'task.progress') {
      return;
    }
    debouncedRefreshFolders(() => void refreshFolders());
    debouncedRefreshTaxonomy(() => void refreshTaxonomy());
  }

  return {
    view, query, skeleton, details, total, totalSize, viewTitle, loading, windowLoading, selection, folders, categories, tagList, trashTotal, rootCount, uncategorizedCount, untaggedCount, library, thumbSize, previewId, toast, importProgress, taskBacklog, sidebarVisible,
    isTrash, canGoBack, canGoForward, currentFolderPath, selectedItems, primarySelected, previewItem, previewIndex, previewNavId, flatFolders, categoryOptions, thumbSizes,
    init, setView, goBack, goForward, toggleSidebar, setQuery, resetList, ensureWindow, reloadSkeleton,
    select, selectAll, clearSelection,
    updateItem, trashSelected, restoreSelected, clearTrash, importBegin, importPaths,
    folderCreate, folderRename, folderDelete, refreshFolders,
    refreshTaxonomy, categoryCreate, categoryRename, categoryDelete, tagCreate, tagRename, tagDelete, addCategoryToSelected, addTagToSelected, moveSelectedToFolder,
    openPreview, closePreview, navigatePreview, showToast, applyEvent,
  };
});

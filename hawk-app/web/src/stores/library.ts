// Pinia 主 store：视图/查询/列表/选择集/文件夹树，以及全部业务 action。
// 组件不直接调 api，一切经 action；SSE 事件经 applyEvent 分发。
import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '../api/endpoints';
import { ApiError } from '../api/client';
import type { FolderNode, Item, ItemListRequest, LibraryInfo, QueryState, ViewState } from '../types';

const PAGE_SIZE = 100;

const ERROR_TEXT: Record<string, string> = {
  FILE_EXISTS: '同名文件或文件夹已存在',
  ITEM_NOT_FOUND: '素材不存在或已被移除',
  FOLDER_NOT_FOUND: '文件夹不存在',
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
  const loading = ref(false);
  const endReached = ref(false);
  const selection = ref<string[]>([]);
  const folders = ref<FolderNode | null>(null);
  const library = ref<LibraryInfo | null>(null);
  const thumbSize = ref(160);
  const previewId = ref<string | null>(null);
  const toast = ref<string | null>(null);

  // ---- getters ----
  const isTrash = computed(() => view.value.kind === 'trash');
  const currentFolderPath = computed(() => (view.value.kind === 'folder' ? view.value.path : null));
  const selectedItems = computed(
    () => selection.value.map((id) => items.value.find((i) => i.id === id)).filter((i): i is Item => !!i),
  );
  const primarySelected = computed(() => selectedItems.value.at(-1) ?? null);
  const previewItem = computed(() => items.value.find((i) => i.id === previewId.value) ?? null);
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

  // ---- 内部 ----
  const debouncedRefresh = debounce(200);
  const debouncedRefreshFolders = debounce(300);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  function buildListParams(offset: number, limit: number): ItemListRequest {
    return {
      keywords: query.value.keywords.length > 0 ? query.value.keywords : undefined,
      star: query.value.star,
      order_by: query.value.orderBy,
      order: query.value.order,
      in_trash: isTrash.value || undefined,
      folders: currentFolderPath.value ? [currentFolderPath.value] : undefined,
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
    await refreshFolders();
    restoreView();
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
      view.value = parsed;
    } catch {
      // 损坏的持久化数据忽略
    }
  }

  function folderExists(path: string): boolean {
    const walk = (node: FolderNode): boolean => node.path === path || node.children.some(walk);
    return folders.value ? walk(folders.value) : false;
  }

  function setView(v: ViewState) {
    view.value = v;
    localStorage.setItem(viewStorageKey(), JSON.stringify(v));
    clearSelection();
    void resetList();
  }

  function setQuery(patch: Partial<QueryState>) {
    Object.assign(query.value, patch);
    void resetList();
  }

  async function resetList() {
    items.value = [];
    endReached.value = false;
    total.value = 0;
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
      endReached.value = items.value.length >= total.value;
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
      const idx = items.value.findIndex((i) => i.id === id);
      if (idx >= 0) {
        items.value[idx] = updated;
      }
    } catch (e) {
      showToast(errorText(e));
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

  /** 拖拽导入：逐个 itemAddByPath，目标文件夹取当前视图 */
  async function importPaths(paths: string[]) {
    if (paths.length === 0) {
      return;
    }
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
    }
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
        setView({ kind: 'all' });
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
        const updated = payload as Item;
        const idx = items.value.findIndex((i) => i.id === updated.id);
        if (idx >= 0) {
          // 移动到别的文件夹后不再属于当前文件夹视图 → 从列表移除
          const folders = updated.folders ?? [];
          if (currentFolderPath.value && !folders.includes(currentFolderPath.value)) {
            items.value.splice(idx, 1);
            selection.value = selection.value.filter((s) => s !== updated.id);
            total.value = Math.max(0, total.value - 1);
          } else {
            items.value[idx] = updated;
          }
        } else if (isTrash.value) {
          debouncedRefresh(() => void refresh());
        }
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
  }

  return {
    view, query, items, total, loading, endReached, selection, folders, library, thumbSize, previewId, toast,
    isTrash, currentFolderPath, selectedItems, primarySelected, previewItem, previewNavId, flatFolders,
    init, setView, setQuery, resetList, fetchMore, refresh,
    select, selectAll, clearSelection,
    updateItem, trashSelected, restoreSelected, clearTrash, importPaths,
    folderCreate, folderRename, folderDelete, refreshFolders,
    openPreview, closePreview, navigatePreview, showToast, applyEvent,
  };
});

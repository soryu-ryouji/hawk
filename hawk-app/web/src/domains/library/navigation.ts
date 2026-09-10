// 视图导航与排序偏好：从主 store 拆出的独立状态机（依赖注入，便于单测）。
// 职责：视图历史栈（setView/correctView/goBack/goForward）、视图记忆（localStorage 按库路径隔离）、
// 排序偏好（scope 解析、沿父链继承在 viewLogic.resolveSort、持久化与失败回拉）。
// 引用规则不变：本模块不反向依赖主 store，状态 ref 与副作用回调全部由主 store 注入。
import { computed, ref, type Ref } from 'vue';
import { api } from '@/shared/api/endpoints';
import { loadJSON, saveJSON, STORAGE_KEYS } from '@/shared/lib/persist';
import { resolveSort } from './logic/viewLogic';
import type { LibraryInfo, QueryState, ViewPrefs, ViewState } from '@/shared/types';

/** restoreView 的存在性校验（文件夹/分类/标签数据在 taxonomy store，由组件层注入，保持引用方向 DAG） */
export interface ViewValidators {
  folderExists(path: string): boolean;
  categoryExists(name: string): boolean;
  tagExists(name: string): boolean;
}

/** 主 store 注入的依赖：状态 ref 与副作用回调（不反向依赖 store） */
export interface ViewNavigationDeps {
  view: Ref<ViewState>;
  query: Ref<QueryState>;
  viewPrefs: Ref<ViewPrefs>;
  viewerMode: Ref<boolean>;
  library: Ref<LibraryInfo | null>;
  clearSelection: () => void;
  resetList: () => void;
  showToast: (message: string) => void;
}

export function createViewNavigation(deps: ViewNavigationDeps) {
  const { view, query, viewPrefs, viewerMode, library, clearSelection, resetList, showToast } = deps;

  /** 浏览历史（会话内）：setView 压入，前进/后退在栈内移动 */
  const viewHistory = ref<ViewState[]>([]);
  const historyIndex = ref(-1);
  const canGoBack = computed(() => historyIndex.value > 0);
  const canGoForward = computed(() => historyIndex.value >= 0 && historyIndex.value < viewHistory.value.length - 1);

  /** 换库/初始化：历史栈以当前视图为唯一起点 */
  function resetHistory() {
    viewHistory.value = [view.value];
    historyIndex.value = 0;
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

  return {
    loadViewPrefs,
    restoreView,
    applySortForView,
    resetHistory,
    setView,
    correctView,
    goBack,
    goForward,
    canGoBack,
    canGoForward,
    persistSortForCurrentView,
    resetSort,
  };
}

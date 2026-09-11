// 分类维度 store：文件夹树 / 分类 / 标签 / 侧栏计数，及其 CRUD 与 SSE 刷新。
// 引用方向（DAG）：本 store 可读主 store（view/correctView/showToast）；主 store 不得反向引用——
// init/SSE 的跨 store 编排由组件层（App.vue）负责，restoreView 的校验经 validators 参数注入。
import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '@/shared/api/endpoints';
import { addUnlockTicket, dropUnlockTicket, hasUnlockTicket } from '@/shared/api/client';
import { debounce, errorText } from '@/shared/lib/storeUtil';
import { registerTaxonomyHooks, useLibraryStore } from '@/domains/library';
import type { CategoryInfo, FolderNode, GlobalFilter, Locks, TagInfo } from '@/shared/types';

export const useTaxonomyStore = defineStore('taxonomy', () => {
  const library = useLibraryStore();

  // ---- state ----
  const folders = ref<FolderNode | null>(null);
  const categories = ref<CategoryInfo[]>([]);
  const tagList = ref<TagInfo[]>([]);
  const trashTotal = ref(0);
  /** 全部素材计数（应用隐藏排除后；不再用文件夹树根节点计数——那是未过滤口径） */
  const allCount = ref(0);
  // 侧栏智能条目计数（limit:1 只取 total）
  const rootCount = ref(0);
  const uncategorizedCount = ref(0);
  const untaggedCount = ref(0);
  /** 全局列表隐藏集（与主 store 共用同一份：变更经 applyGlobalFilter 同步两侧） */
  const globalFilter = ref<GlobalFilter>({ folders: [], categories: [], tags: [] });

  // ---- 隐藏集 ----
  const hiddenFolderSet = computed(() => new Set(globalFilter.value.folders));
  const hiddenCategorySet = computed(() => new Set(globalFilter.value.categories));
  const hiddenTagSet = computed(() => new Set(globalFilter.value.tags));

  /** 维度是否被标记为全局列表隐藏（文件夹为精确路径命中；子树继承由查询层前缀匹配承担） */
  function isHidden(kind: 'folder' | 'category' | 'tag', name: string): boolean {
    const set = kind === 'folder' ? hiddenFolderSet.value : kind === 'category' ? hiddenCategorySet.value : hiddenTagSet.value;
    return set.has(name);
  }

  /** 隐藏集落位：本 store 与主 store 同步同一份（纯状态，无后续动作；首屏加载不应触发重查） */
  function applyGlobalFilter(gf: GlobalFilter) {
    globalFilter.value = gf;
    library.setGlobalFilter(gf);
  }

  /** 隐藏集变更的联动收尾：成员与计数均已变化 → 重查骨架 + 防抖刷新计数 */
  function onGlobalFilterUpdated(gf: GlobalFilter) {
    applyGlobalFilter(gf);
    debouncedRefreshTaxonomy(() => void refreshTaxonomy());
    void library.reloadSkeleton();
  }

  async function refreshGlobalFilter() {
    try {
      applyGlobalFilter(await api.globalFilterList());
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  /** 标记/取消隐藏（右键菜单与设置面板共用）：本地先行对齐（SSE 回声到达后幂等重放） */
  async function setHidden(kind: 'folder' | 'category' | 'tag', name: string, hidden: boolean) {
    try {
      await api.globalFilterSet(kind, name, hidden);
      await refreshGlobalFilter();
      onGlobalFilterUpdated(globalFilter.value);
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  // ---- 锁（.hawk/locks.toml，服务端强制；解锁票据由 client 层独立持有） ----
  const locks = ref<Locks>({ folders: [], categories: [], tags: [] });
  const lockFolderSet = computed(() => new Set(locks.value.folders));
  const lockCategorySet = computed(() => new Set(locks.value.categories));
  const lockTagSet = computed(() => new Set(locks.value.tags));

  /** 锁管理对话框状态（右键菜单发起：设锁/改密/移除，admin）；解锁不走对话框——
   *  进入锁定视图时内容区由主 store 的 lockedView 驱动锁占位界面（密码输入） */
  const lockDialog = ref<{ mode: 'set' | 'change' | 'remove'; dimension: 'folder' | 'category' | 'tag'; name: string } | null>(null);

  /** 覆盖该维度的锁条目：文件夹取自身或最近锁定祖先（锁条目名）；分类/标签为自身。无锁返回 null */
  function coveringLock(kind: 'folder' | 'category' | 'tag', name: string): string | null {
    if (kind === 'folder') {
      let cur: string | null = name;
      while (cur !== null) {
        if (lockFolderSet.value.has(cur)) {
          return cur;
        }
        cur = cur.includes('/') ? cur.slice(0, cur.lastIndexOf('/')) : null;
      }
      return null;
    }
    const set = kind === 'category' ? lockCategorySet.value : lockTagSet.value;
    return set.has(name) ? name : null;
  }

  /** 该维度是否被锁覆盖（树/面板锁图标与点击拦截用） */
  function isLocked(kind: 'folder' | 'category' | 'tag', name: string): boolean {
    return coveringLock(kind, name) !== null;
  }

  /** 本客户端是否已解锁该维度（票据持有即视为解锁；文件夹按覆盖锁条目判定） */
  function isUnlocked(kind: 'folder' | 'category' | 'tag', name: string): boolean {
    const entry = coveringLock(kind, name);
    return entry !== null && hasUnlockTicket(kind, entry);
  }

  /** 锁集落位 + 联动：列表成员与侧栏计数均可能变化 → 重查骨架与计数 */
  function onLocksUpdated(next: Locks) {
    locks.value = next;
    debouncedRefreshTaxonomy(() => void refreshTaxonomy());
    void library.reloadSkeleton();
  }

  async function refreshLocks() {
    try {
      locks.value = await api.lockList();
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  /** 解锁（任何 token 可用）：登记票据 + 退锁占位界面 + 原地重查当前视图（导航早已发生，
   *  密码正确即从锁界面替换为实际内容） */
  async function unlock(kind: 'folder' | 'category' | 'tag', name: string, password: string): Promise<boolean> {
    try {
      const res = await api.lockUnlock(kind, name, password);
      addUnlockTicket(kind, name, res.unlock_token);
      library.setLockedView(null);
      void library.reloadSkeleton();
      refreshTaxonomySoon();
      return true;
    } catch (e) {
      library.showToast(errorText(e));
      return false;
    }
  }

  /** 锁定回去：仅丢弃本客户端票据（不影响其他已解锁的客户端） */
  function relock(kind: 'folder' | 'category' | 'tag', name: string) {
    const entry = coveringLock(kind, name);
    if (entry) {
      dropUnlockTicket(kind, entry);
      void library.reloadSkeleton();
      refreshTaxonomySoon();
    }
  }

  /** 设锁/改密（admin）：成功后刷新锁集（locks.changed 事件也会到达，幂等） */
  async function lockSet(kind: 'folder' | 'category' | 'tag', name: string, password: string, oldPassword?: string): Promise<boolean> {
    try {
      await api.lockSet(kind, name, password, oldPassword);
      await refreshLocks();
      onLocksUpdated(locks.value);
      return true;
    } catch (e) {
      library.showToast(errorText(e));
      return false;
    }
  }

  /** 解除锁（admin，需密码）：全部内容即刻无需解锁可见 */
  async function lockRemove(kind: 'folder' | 'category' | 'tag', name: string, password: string): Promise<boolean> {
    try {
      await api.lockRemove(kind, name, password);
      await refreshLocks();
      onLocksUpdated(locks.value);
      return true;
    } catch (e) {
      library.showToast(errorText(e));
      return false;
    }
  }

  /** 隐藏排除参数（全局类视图计数查询用；与主 store listParams 同口径） */
  function excludeParams() {
    const gf = globalFilter.value;
    return {
      exclude_folders: gf.folders.length > 0 ? gf.folders : undefined,
      exclude_categories: gf.categories.length > 0 ? gf.categories : undefined,
      exclude_tags: gf.tags.length > 0 ? gf.tags : undefined,
    };
  }

  // ---- getters ----
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

  function folderExists(path: string): boolean {
    const walk = (node: FolderNode): boolean => node.path === path || node.children.some(walk);
    return folders.value ? walk(folders.value) : false;
  }

  function categoryExists(name: string): boolean {
    return categories.value.some((c) => c.name === name);
  }

  function tagExists(name: string): boolean {
    return tagList.value.some((t) => t.name === name);
  }

  // ---- 加载 ----

  async function refreshFolders() {
    try {
      folders.value = await api.folderList();
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  async function refreshTaxonomy() {
    try {
      const excludes = excludeParams();
      const [categoryList, tags, trash, all, root, uncategorized, untagged] = await Promise.all([
        api.categoryList(),
        api.tagList(),
        api.itemList({ in_trash: true, limit: 1 }),
        api.itemList({ ...excludes, limit: 1 }),
        api.itemList({ folders: [''], folders_exact: true, ...excludes, limit: 1 }),
        api.itemList({ without_categories: true, ...excludes, limit: 1 }),
        api.itemList({ without_tags: true, ...excludes, limit: 1 }),
      ]);
      categories.value = categoryList;
      tagList.value = tags;
      trashTotal.value = Number(trash.total);
      allCount.value = Number(all.total);
      rootCount.value = Number(root.total);
      uncategorizedCount.value = Number(uncategorized.total);
      untaggedCount.value = Number(untagged.total);
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  /** 首屏/换库加载（组件层编排，先于主 store init：restoreView 的校验依赖本 store 数据）。
   *  隐藏集必须先于计数就绪：refreshTaxonomy 的计数查询带 exclude 参数，并发跑会按未过滤口径算错 */
  async function refreshAll() {
    await Promise.all([refreshGlobalFilter(), refreshLocks()]);
    await Promise.all([refreshFolders(), refreshTaxonomy()]);
  }

  // ---- 文件夹写操作 ----
  async function folderCreate(parentPath: string, name: string) {
    try {
      await api.folderCreate(name, parentPath || undefined);
      await refreshFolders();
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  async function folderRename(path: string, name: string) {
    try {
      await api.folderUpdate(path, { name });
      await refreshFolders();
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  async function folderDelete(path: string) {
    try {
      await api.folderDelete(path);
      await refreshFolders();
      if (library.currentFolderPath === path || library.currentFolderPath?.startsWith(path + '/')) {
        library.correctView({ kind: 'all' });
      }
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  // ---- 分类/标签写操作 ----
  async function categoryCreate(name: string) {
    try {
      await api.categoryCreate(name);
      await refreshTaxonomy();
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  async function categoryRename(name: string, newName: string) {
    try {
      await api.categoryUpdate(name, newName);
      await refreshTaxonomy();
      // 当前视图正在该分类下 → 跟随新名字（局部变量收窄判别联合）
      const v = library.view;
      if (v.kind === 'category' && v.name === name) {
        library.correctView({ kind: 'category', name: newName });
      }
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  async function categoryDelete(name: string) {
    try {
      await api.categoryDelete(name);
      await refreshTaxonomy();
      const v = library.view;
      if (v.kind === 'category' && v.name === name) {
        library.correctView({ kind: 'all' });
      }
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  async function tagCreate(name: string) {
    try {
      await api.tagCreate(name);
      await refreshTaxonomy();
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  async function tagRename(name: string, newName: string) {
    try {
      await api.tagUpdate(name, newName);
      await refreshTaxonomy();
      if (library.view.kind === 'tag' && library.view.name === name) {
        library.correctView({ kind: 'tag', name: newName });
      }
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  async function tagDelete(name: string) {
    try {
      await api.tagDelete(name);
      await refreshTaxonomy();
      const v = library.view;
      if (v.kind === 'tag' && v.name === name) {
        library.correctView({ kind: 'all' });
      }
    } catch (e) {
      library.showToast(errorText(e));
    }
  }

  // ---- 刷新钩子（注册到主 store：SSE 事件与本地批量操作的分类维度刷新经此转发，内部防抖） ----
  const debouncedRefreshFolders = debounce(300);
  const debouncedRefreshTaxonomy = debounce(300);

  /** 防抖刷新入口（公共）：SSE hooks、服务层批量写操作、跨域调用统一走这两个，
   *  爆发期合并为 300ms 内一次；refreshAll/refreshFolders 等裸函数仅供需等待结果的编排使用 */
  function refreshTaxonomySoon() {
    debouncedRefreshTaxonomy(() => void refreshTaxonomy());
  }

  function refreshFoldersSoon() {
    debouncedRefreshFolders(() => void refreshFolders());
  }

  registerTaxonomyHooks({
    refreshTaxonomy: refreshTaxonomySoon,
    refreshFolders: refreshFoldersSoon,
    onGlobalFilterChanged: (filter) => onGlobalFilterUpdated(filter),
    onLocksChanged: (next) => onLocksUpdated(next),
    onLockedView: () => {
      // 查询 403（进入未解锁的锁定维度视图 / 锁定回去 / 被其他客户端刚上锁）：
      // 解析覆盖锁条目（文件夹可能是祖先）→ 主 store 置 lockedView，内容区切锁占位界面
      const v = library.view;
      if (v.kind === 'folder') {
        const entry = coveringLock('folder', v.path);
        if (entry) library.setLockedView({ dimension: 'folder', name: entry });
      } else if (v.kind === 'category' || v.kind === 'tag') {
        const dim = v.kind === 'category' ? 'category' : 'tag';
        if (coveringLock(dim, v.name)) library.setLockedView({ dimension: dim, name: v.name });
      }
    },
  });

  return {
    folders,
    categories,
    tagList,
    trashTotal,
    allCount,
    rootCount,
    uncategorizedCount,
    untaggedCount,
    globalFilter,
    flatFolders,
    categoryOptions,
    folderExists,
    categoryExists,
    tagExists,
    isHidden,
    setHidden,
    locks,
    isLocked,
    isUnlocked,
    coveringLock,
    lockDialog,
    unlock,
    relock,
    lockSet,
    lockRemove,
    refreshLocks,
    refreshGlobalFilter,
    refreshFolders,
    refreshFoldersSoon,
    refreshTaxonomy,
    refreshTaxonomySoon,
    refreshAll,
    folderCreate,
    folderRename,
    folderDelete,
    categoryCreate,
    categoryRename,
    categoryDelete,
    tagCreate,
    tagRename,
    tagDelete,
  };
});

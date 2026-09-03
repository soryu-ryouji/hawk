// 分类维度 store：文件夹树 / 分类 / 标签 / 侧栏计数，及其 CRUD 与 SSE 刷新。
// 引用方向（DAG）：本 store 可读主 store（view/correctView/showToast）；主 store 不得反向引用——
// init/SSE 的跨 store 编排由组件层（App.vue）负责，restoreView 的校验经 validators 参数注入。
import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '../api/endpoints';
import { debounce, errorText } from './util';
import { registerTaxonomyHooks, useLibraryStore } from './library';
import type { CategoryInfo, FolderNode, TagInfo } from '../types';

export const useTaxonomyStore = defineStore('taxonomy', () => {
  const library = useLibraryStore();

  // ---- state ----
  const folders = ref<FolderNode | null>(null);
  const categories = ref<CategoryInfo[]>([]);
  const tagList = ref<TagInfo[]>([]);
  const trashTotal = ref(0);
  // 侧栏智能条目计数（limit:1 只取 total）
  const rootCount = ref(0);
  const uncategorizedCount = ref(0);
  const untaggedCount = ref(0);

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
      library.showToast(errorText(e));
    }
  }

  /** 首屏/换库加载（组件层编排，先于主 store init：restoreView 的校验依赖本 store 数据） */
  async function refreshAll() {
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

  registerTaxonomyHooks({
    refreshTaxonomy: () => debouncedRefreshTaxonomy(() => void refreshTaxonomy()),
    refreshFolders: () => debouncedRefreshFolders(() => void refreshFolders()),
  });

  return {
    folders, categories, tagList, trashTotal, rootCount, uncategorizedCount, untaggedCount,
    flatFolders, categoryOptions, folderExists, categoryExists, tagExists,
    refreshFolders, refreshTaxonomy, refreshAll,
    folderCreate, folderRename, folderDelete,
    categoryCreate, categoryRename, categoryDelete, tagCreate, tagRename, tagDelete,
  };
});

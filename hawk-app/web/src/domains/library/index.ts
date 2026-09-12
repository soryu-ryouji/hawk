// 素材库域公共出口（域外唯一可 import 的文件，依赖规则由 eslint boundaries 强制）：
// store、写操作服务、跨域需要的纯逻辑与组件。域内文件互相引用走相对路径。
export { useLibraryStore, registerTaxonomyHooks } from './store';
export type { TaxonomyHooks } from './store';
export {
  updateItem,
  trashSelected,
  deleteLocation,
  restoreSelected,
  clearTrash,
  renameLibrary,
  setPeriodicRescan,
  rescanFiles,
  refreshCache,
  cleanupIndex,
  addCategoryToSelected,
  addTagToSelected,
  moveSelectedToFolder,
  setStarForSelected,
  removeTagFromSelected,
  removeCategoryFromSelected,
} from './actions';
export { itemKey, splitKey, displayPath, TRASH_PREFIX, selectionUniqueIds } from './logic/viewLogic';
export { gridNavRows, markKeyboardNavScroll, moveGridSelection } from './logic/useGridNav';
export { default as ItemGrid } from './components/ItemGrid.vue';
export { default as Inspector } from './components/inspector/Inspector.vue';
export { default as FilterBar } from './components/FilterBar.vue';
export { default as SearchBox } from './components/SearchBox.vue';

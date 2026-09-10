// 分类维度域公共出口（域外唯一可 import 的文件）：文件夹树/分类/标签的 store 与
// 被其他域复用的对话框组件。TaxonomyRow/Sidebar 内部组件不对外。
export { useTaxonomyStore } from './store';
export { default as Sidebar } from './components/Sidebar.vue';
export { default as FolderTreePicker } from './components/FolderTreePicker.vue';
export { default as FolderPickerDialog } from './components/FolderPickerDialog.vue';
export { default as CategoryPickerDialog } from './components/CategoryPickerDialog.vue';
export { default as TagEditor } from './components/TagEditor.vue';

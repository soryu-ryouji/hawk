// 导入域公共出口：批量导入 store、拖拽导入 composable（App 装配层挂载）与重复策略对话框。
export { useImporterStore } from './store';
export { useDragImport } from './logic/useDragImport';
export { default as ImportDuplicateDialog } from './components/ImportDuplicateDialog.vue';

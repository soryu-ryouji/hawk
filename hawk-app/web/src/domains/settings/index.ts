// 设置域公共出口：设置对话框（App 装配层挂载）与更新状态机。
export { useUpdater, startupAutoCheck } from './updater';
export { useLanSettingsStore } from './lanSettings';
export { default as SettingsDialog } from './components/SettingsDialog.vue';
export { default as SettingsRemote } from './components/SettingsRemote.vue';
export { default as SettingsLanConnect } from './components/SettingsLanConnect.vue';
export { default as LanPairRequestPrompt } from './components/LanPairRequestPrompt.vue';

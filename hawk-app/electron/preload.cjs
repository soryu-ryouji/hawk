// preload：只暴露三个白名单通道，业务数据不经 IPC。
const { contextBridge, ipcRenderer, webUtils } = require('electron');

contextBridge.exposeInMainWorld('hawkShell', {
  // 运行平台（darwin/win32/linux），前端据此区分系统称呼
  platform: process.platform,
  /** 更换素材库：弹目录选择框，主进程重启 server 并重载窗口 */
  selectLibrary: () => ipcRenderer.invoke('hawk:select-library'),
  /** 在系统文件管理器中显示库内文件（相对路径） */
  showInFinder: (relPath) => ipcRenderer.invoke('hawk:show-in-finder', relPath),
  /** 拖拽导入时取文件绝对路径（Electron webUtils） */
  getPathForFile: (file) => webUtils.getPathForFile(file),
});

// preload：只暴露白名单通道，业务数据不经 IPC。
const { contextBridge, ipcRenderer, webUtils } = require('electron');

contextBridge.exposeInMainWorld('hawkShell', {
  // 运行平台（darwin/win32/linux），前端据此区分系统称呼
  platform: process.platform,
  /** 更换素材库：弹目录选择框，主进程重启 server 并重载窗口 */
  selectLibrary: () => ipcRenderer.invoke('hawk:select-library'),
  /** 在系统文件管理器中显示库内文件（相对路径） */
  showInFinder: (relPath) => ipcRenderer.invoke('hawk:show-in-finder', relPath),
  /** 复制库内文件的绝对路径到剪贴板 */
  copyPath: (relPath) => ipcRenderer.invoke('hawk:copy-path', relPath),
  /** 复制图片文件本身到剪贴板 */
  copyImage: (relPath) => ipcRenderer.invoke('hawk:copy-image', relPath),
  /** 拖拽导入时取文件绝对路径（Electron webUtils） */
  getPathForFile: (file) => webUtils.getPathForFile(file),
  /** 窗口最小化（自绘标题栏按钮） */
  minimizeWindow: () => ipcRenderer.invoke('hawk:win-minimize'),
  /** 最大化/还原切换，返回切换后的最大化状态 */
  toggleMaximizeWindow: () => ipcRenderer.invoke('hawk:win-maximize-toggle'),
  /** 关闭窗口 */
  closeWindow: () => ipcRenderer.invoke('hawk:win-close'),
  /** 订阅最大化状态变化（含 Aero Snap 等系统途径），返回退订函数 */
  onWindowMaximized: (cb) => {
    const listener = (_event, maximized) => cb(maximized);
    ipcRenderer.on('hawk:win-maximized', listener);
    return () => ipcRenderer.removeListener('hawk:win-maximized', listener);
  },
});

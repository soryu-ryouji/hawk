// preload：只暴露白名单通道，业务数据不经 IPC。
const { contextBridge, ipcRenderer, webUtils } = require('electron');

contextBridge.exposeInMainWorld('hawkShell', {
  // 运行平台（darwin/win32/linux），前端据此区分系统称呼
  platform: process.platform,
  /** 更换素材库：弹目录选择框并拉起新 server；就绪经 onServerStarted 通知 */
  selectLibrary: () => ipcRenderer.invoke('hawk:select-library'),
  /** 本机打开过的素材库历史（最近在前，含目录存在性）与当前库路径 */
  listLibraries: () => ipcRenderer.invoke('hawk:list-libraries'),
  /** 打开历史素材库（仅限历史记录内的路径）；就绪经 onServerStarted 通知 */
  openLibrary: (path) => ipcRenderer.invoke('hawk:open-library', path),
  /** 复制库内文件的绝对路径到剪贴板 */
  copyPath: (relPath) => ipcRenderer.invoke('hawk:copy-path', relPath),
  /** 复制图片文件本身到剪贴板 */
  copyImage: (relPath) => ipcRenderer.invoke('hawk:copy-image', relPath),
  /** 局域网查看设置：读取 [web] 配置与本机局域网地址 */
  getLanSettings: () => ipcRenderer.invoke('hawk:get-lan-settings'),
  /** 保存 [web] 配置并重启 hawk-server（失败自动回滚），返回 { ok, error? }；重启后 onServerStarted 带新地址到达 */
  saveLanSettings: (web) => ipcRenderer.invoke('hawk:save-lan-settings', web),
  /** 真正退出应用（启动错误屏用；区别于 closeWindow 的隐藏到托盘） */
  quitApp: () => ipcRenderer.invoke('hawk:quit-app'),
  /** 在系统文件管理器中显示库内文件（相对路径） */
  showInFinder: (relPath) => ipcRenderer.invoke('hawk:show-in-finder', relPath),
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
  /** 订阅 server 扫描进度（应用内启动屏用）：{ phase, processed, total }，total=0 表示不定态 */
  onServerProgress: (cb) => {
    const listener = (_event, progress) => cb(progress);
    ipcRenderer.on('hawk:server-progress', listener);
    return () => ipcRenderer.removeListener('hawk:server-progress', listener);
  },
  /** 订阅 server 即将重启（换库/应用设置重启）：旧 server 已停，前端应立即切启动屏，就绪经 onServerStarted 到达 */
  onServerRestarting: (cb) => {
    const listener = () => cb();
    ipcRenderer.on('hawk:server-restarting', listener);
    return () => ipcRenderer.removeListener('hawk:server-restarting', listener);
  },
  /** 订阅 server 就绪：{ address, token }（冷启动/换库/应用设置重启都会到达，需重配 API 并重启数据） */
  onServerStarted: (cb) => {
    const listener = (_event, conn) => cb(conn);
    ipcRenderer.on('hawk:server-started', listener);
    return () => ipcRenderer.removeListener('hawk:server-started', listener);
  },
  /** 订阅 server 启动/运行失败：{ message } */
  onServerError: (cb) => {
    const listener = (_event, error) => cb(error);
    ipcRenderer.on('hawk:server-error', listener);
    return () => ipcRenderer.removeListener('hawk:server-error', listener);
  },
});

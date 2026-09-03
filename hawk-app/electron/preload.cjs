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
  /** 本机局域网 IPv4 地址列表（设置面板展示用；LAN 配置读写走 REST app/lan） */
  lanAddresses: () => ipcRenderer.invoke('hawk:lan-addresses'),
  /** 真正退出应用（启动错误屏用；区别于 closeWindow 的隐藏到托盘） */
  quitApp: () => ipcRenderer.invoke('hawk:quit-app'),
  /** 当前应用版本与构建 sha（sha='dev' 表示无构建标识，如开发态） */
  getAppVersion: () => ipcRenderer.invoke('hawk:app-version'),
  /** 检查更新（stable=latest 正式版比 semver；nightly=滚动预发布比构建 sha）；无更新返回 null，失败 reject */
  checkUpdate: (channel) => ipcRenderer.invoke('hawk:update-check', channel),
  /** 下载并校验上次检查到的更新（进度经 onUpdateProgress 推送；已就绪时幂等） */
  downloadUpdate: () => ipcRenderer.invoke('hawk:update-download'),
  /** 重启并安装已下载的更新（成功后应用退出，不再返回） */
  installUpdate: () => ipcRenderer.invoke('hawk:update-install'),
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
  /** 订阅更新包下载进度：{ phase: 'downloading', received, total } | { phase: 'verifying' } | { phase: 'ready' } */
  onUpdateProgress: (cb) => {
    const listener = (_event, progress) => cb(progress);
    ipcRenderer.on('hawk:update-progress', listener);
    return () => ipcRenderer.removeListener('hawk:update-progress', listener);
  },
});

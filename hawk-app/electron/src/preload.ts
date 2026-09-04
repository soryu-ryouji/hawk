// preload：只暴露白名单通道（形状见 ipc-contract.ts 的 HawkShell），业务数据不经 IPC。
import { contextBridge, ipcRenderer, webUtils, type IpcRendererEvent } from 'electron';
import { IPC, type HawkShell } from './ipc-contract';

/** on* 订阅通道的通用接线：注册监听并返回退订函数 */
function subscribe<T extends unknown[]>(channel: string, cb: (...args: T) => void): () => void {
  const listener = (_event: IpcRendererEvent, ...args: unknown[]): void => cb(...(args as T));
  ipcRenderer.on(channel, listener);
  return () => ipcRenderer.removeListener(channel, listener);
}

const shell: HawkShell = {
  // 运行平台（darwin/win32/linux），前端据此区分系统称呼
  platform: process.platform,
  selectLibrary: () => ipcRenderer.invoke(IPC.selectLibrary),
  listLibraries: () => ipcRenderer.invoke(IPC.listLibraries),
  openLibrary: (path) => ipcRenderer.invoke(IPC.openLibrary, path),
  removeLibrary: (path) => ipcRenderer.invoke(IPC.removeLibrary, path),
  copyPath: (relPath) => ipcRenderer.invoke(IPC.copyPath, relPath),
  lanAddresses: () => ipcRenderer.invoke(IPC.lanAddresses),
  quitApp: () => ipcRenderer.invoke(IPC.quitApp),
  getAppVersion: () => ipcRenderer.invoke(IPC.appVersion),
  checkUpdate: (channel) => ipcRenderer.invoke(IPC.updateCheck, channel),
  getUpdateChannel: () => ipcRenderer.invoke(IPC.updateChannelGet),
  setUpdateChannel: (channel) => ipcRenderer.invoke(IPC.updateChannelSet, channel),
  downloadUpdate: () => ipcRenderer.invoke(IPC.updateDownload),
  cancelUpdate: () => ipcRenderer.invoke(IPC.updateCancel),
  installUpdate: () => ipcRenderer.invoke(IPC.updateInstall),
  showInFinder: (relPath) => ipcRenderer.invoke(IPC.showInFinder, relPath),
  getCacheDir: () => ipcRenderer.invoke(IPC.cacheDirGet),
  pickCacheDir: () => ipcRenderer.invoke(IPC.cacheDirPick),
  changeCacheDir: (path) => ipcRenderer.invoke(IPC.cacheDirChange, path),
  getPathForFile: (file) => webUtils.getPathForFile(file),
  minimizeWindow: () => ipcRenderer.invoke(IPC.winMinimize),
  toggleMaximizeWindow: () => ipcRenderer.invoke(IPC.winMaximizeToggle),
  closeWindow: () => ipcRenderer.invoke(IPC.winClose),
  onUpdateProgress: (cb) => subscribe(IPC.updateProgress, cb),
  onServerStarted: (cb) => subscribe(IPC.serverStarted, cb),
  getServerConn: () => ipcRenderer.invoke(IPC.serverConn),
  onServerError: (cb) => subscribe(IPC.serverError, cb),
  onServerRestarting: (cb) => subscribe(IPC.serverRestarting, cb),
  onServerProgress: (cb) => subscribe(IPC.serverProgress, cb),
  onWindowMaximized: (cb) => subscribe(IPC.winMaximized, cb),
};

contextBridge.exposeInMainWorld('hawkShell', shell);

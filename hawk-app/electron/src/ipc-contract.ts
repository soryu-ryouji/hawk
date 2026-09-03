// IPC 契约：主进程 handler、preload 暴露、web 前端消费的三方对齐点（单一定义）。
// 通道名一律经 IPC 常量引用，不手写字符串；web 端 types.ts 从本文件 re-export。

/** IPC 通道名（invoke/handle 与 send/on 的全集） */
export const IPC = {
  // ---- 渲染进程 → 主进程（ipcRenderer.invoke / ipcMain.handle） ----
  selectLibrary: 'hawk:select-library',
  listLibraries: 'hawk:list-libraries',
  openLibrary: 'hawk:open-library',
  copyPath: 'hawk:copy-path',
  lanAddresses: 'hawk:lan-addresses',
  quitApp: 'hawk:quit-app',
  appVersion: 'hawk:app-version',
  updateCheck: 'hawk:update-check',
  updateDownload: 'hawk:update-download',
  updateInstall: 'hawk:update-install',
  showInFinder: 'hawk:show-in-finder',
  winMinimize: 'hawk:win-minimize',
  winMaximizeToggle: 'hawk:win-maximize-toggle',
  winClose: 'hawk:win-close',
  // ---- 主进程 → 渲染进程（webContents.send / ipcRenderer.on） ----
  winMaximized: 'hawk:win-maximized',
  serverProgress: 'hawk:server-progress',
  serverRestarting: 'hawk:server-restarting',
  serverStarted: 'hawk:server-started',
  serverError: 'hawk:server-error',
  updateProgress: 'hawk:update-progress',
} as const;

/** 素材库历史条目（主进程记录，最近使用在前） */
export interface LibraryHistoryItem {
  path: string;
  /** 目录名（basename） */
  name: string;
  /** 目录仍存在；已删除的历史项展示但不可选 */
  exists: boolean;
}

/** 应用更新信息（主进程查询 GitHub Releases 的结果） */
export interface UpdateInfo {
  channel: 'stable' | 'nightly';
  /** stable：版本号（无 v 前缀）；nightly：目标 commit 短 sha */
  version: string;
  /** Release 说明（nightly 为触发提交信息） */
  notes: string;
  /** Release 页面链接 */
  url: string;
  assetName: string;
  /** 更新包字节数（未知为 0） */
  size: number;
}

/** 更新包下载进度事件 */
export type UpdateProgress =
  | { phase: 'downloading'; received: number; total: number }
  | { phase: 'verifying' }
  | { phase: 'ready' };

/** server 就绪事件负载（冷启动/换库/应用设置重启都会到达，restart 会换端口） */
export interface ServerConn {
  address: string;
  token: string;
}

/** server 索引进度事件负载（total=0 表示不定态） */
export interface ServerProgress {
  phase: string;
  processed: number;
  total: number;
}

/** Electron preload 注入的白名单通道（window.hawkShell；浏览器纯前端调试时不存在） */
export interface HawkShell {
  platform: string;
  /** 更换素材库：弹目录选择框并拉起新 server；就绪经 onServerStarted 通知 */
  selectLibrary(): Promise<boolean>;
  /** 本机打开过的素材库历史与当前库路径 */
  listLibraries(): Promise<{ current: string | null; libraries: LibraryHistoryItem[] }>;
  /** 打开历史素材库（仅限历史记录内的路径） */
  openLibrary(path: string): Promise<boolean>;
  /** 复制库内文件的绝对路径到剪贴板 */
  copyPath(relPath: string): Promise<void>;
  /** 本机局域网 IPv4 地址列表（设置面板展示用；LAN 配置读写走 REST app/lan） */
  lanAddresses(): Promise<string[]>;
  /** 在系统文件管理器中显示库内文件（相对路径） */
  showInFinder(relPath: string): Promise<void>;
  /** 拖拽导入时取文件绝对路径（Electron webUtils） */
  getPathForFile(file: File): string;
  minimizeWindow(): Promise<void>;
  /** 最大化/还原切换，返回切换后的最大化状态 */
  toggleMaximizeWindow(): Promise<boolean>;
  closeWindow(): Promise<void>;
  /** 真正退出应用（启动错误屏用；区别于 closeWindow 的隐藏到托盘） */
  quitApp(): Promise<void>;
  /** 当前应用版本与构建 sha（sha='dev' 表示无构建标识，如开发态） */
  getAppVersion(): Promise<{ version: string; sha: string }>;
  /** 检查更新（stable=latest 正式版比 semver；nightly=滚动预发布比构建 sha）；无更新返回 null */
  checkUpdate(channel: 'stable' | 'nightly'): Promise<UpdateInfo | null>;
  /** 下载并校验上次检查到的更新（进度经 onUpdateProgress 推送；已就绪时幂等） */
  downloadUpdate(): Promise<void>;
  /** 重启并安装已下载的更新（成功后应用退出，不再返回） */
  installUpdate(): Promise<void>;
  /** 订阅更新包下载进度，返回退订函数 */
  onUpdateProgress(cb: (p: UpdateProgress) => void): () => void;
  /** 订阅 server 就绪（携带新地址与 token，需重配 API 并重启数据），返回退订函数 */
  onServerStarted(cb: (conn: ServerConn) => void): () => void;
  /** 订阅 server 启动/运行失败，返回退订函数 */
  onServerError(cb: (error: { message: string }) => void): () => void;
  /** 订阅 server 即将重启（旧 server 已停，应立即切启动屏），返回退订函数 */
  onServerRestarting(cb: () => void): () => void;
  /** 订阅 server 扫描进度（应用内启动屏用），返回退订函数 */
  onServerProgress(cb: (progress: ServerProgress) => void): () => void;
  /** 订阅最大化状态变化（含 Aero Snap 等系统途径），返回退订函数 */
  onWindowMaximized(cb: (maximized: boolean) => void): () => void;
}

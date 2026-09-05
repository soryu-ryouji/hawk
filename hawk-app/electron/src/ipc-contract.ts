// IPC 契约：主进程 handler、preload 暴露、web 前端消费的三方对齐点（单一定义）。
// 通道名一律经 IPC 常量引用，不手写字符串；web 端 types.ts 从本文件 re-export。

/** IPC 通道名（invoke/handle 与 send/on 的全集） */
export const IPC = {
  // ---- 渲染进程 → 主进程（ipcRenderer.invoke / ipcMain.handle） ----
  selectLibrary: 'hawk:select-library',
  listLibraries: 'hawk:list-libraries',
  openLibrary: 'hawk:open-library',
  openLibraryFolder: 'hawk:open-library-folder',
  removeLibrary: 'hawk:remove-library',
  copyPath: 'hawk:copy-path',
  lanAddresses: 'hawk:lan-addresses',
  quitApp: 'hawk:quit-app',
  appVersion: 'hawk:app-version',
  updateChannelGet: 'hawk:update-channel-get',
  updateChannelSet: 'hawk:update-channel-set',
  updateCheck: 'hawk:update-check',
  updateDownload: 'hawk:update-download',
  updateCancel: 'hawk:update-cancel',
  updateInstall: 'hawk:update-install',
  showInFinder: 'hawk:show-in-finder',
  openFolder: 'hawk:open-folder',
  cacheDirGet: 'hawk:cache-dir-get',
  cacheDirPick: 'hawk:cache-dir-pick',
  cacheDirChange: 'hawk:cache-dir-change',
  winMinimize: 'hawk:win-minimize',
  winMaximizeToggle: 'hawk:win-maximize-toggle',
  winClose: 'hawk:win-close',
  // ---- 主进程 → 渲染进程（webContents.send / ipcRenderer.on） ----
  winMaximized: 'hawk:win-maximized',
  serverProgress: 'hawk:server-progress',
  serverRestarting: 'hawk:server-restarting',
  serverStarted: 'hawk:server-started',
  serverConn: 'hawk:server-conn',
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

/** 更新通道（偏好枚举，config.toml 持久化）：stable=正式版（semver 比对）/
 *  nightly=滚动版（构建 sha 比对）/ off=不检查更新 */
export type UpdateChannel = 'stable' | 'nightly' | 'off';

/** 可发起检查的通道（偏好的子集：off 只是不检查，不是可查询的 Release 线） */
export type CheckableChannel = Exclude<UpdateChannel, 'off'>;

/** 应用更新信息（主进程查询 GitHub Releases 的结果） */
export interface UpdateInfo {
  channel: CheckableChannel;
  /** stable：版本号（无 v 前缀）；nightly：目标 commit 短 sha */
  version: string;
  /** Release 说明（nightly 为触发提交信息） */
  notes: string;
  /** Release 页面链接 */
  url: string;
  assetName: string;
  /** 更新包字节数（未知为 0） */
  size: number;
  /** 安装包已在本地且 sha256 校验通过（磁盘缓存命中）：可跳过下载直接安装 */
  downloaded: boolean;
}

/** 用户取消下载的哨兵错误消息（渲染层识别后静默回 available，不进 error 态；
 *  哨兵惯例同 useStartup 的 'UNAUTHORIZED'） */
export const UPDATE_CANCELLED = 'UPDATE_CANCELLED';

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
  /** 在系统文件管理器中打开素材库目录（仅限历史记录内的路径） */
  openLibraryFolder(path: string): Promise<void>;
  /** 从历史记录移除一条素材库（不动目录本身），返回移除后的列表 */
  removeLibrary(path: string): Promise<{ current: string | null; libraries: LibraryHistoryItem[] }>;
  /** 复制库内文件的绝对路径到剪贴板 */
  copyPath(relPath: string): Promise<void>;
  /** 本机局域网 IPv4 地址列表（设置面板展示用；LAN 配置读写走 REST app/lan） */
  lanAddresses(): Promise<string[]>;
  /** 在系统文件管理器中显示库内文件（相对路径） */
  showInFinder(relPath: string): Promise<void>;
  /** 在系统文件管理器中打开库内文件夹本身（区别 showInFinder 的「定位到父级并选中」） */
  openFolder(relPath: string): Promise<void>;
  /** 当前缓存父目录（isDefault=true 表示系统默认路径） */
  getCacheDir(): Promise<{ current: string; isDefault: boolean }>;
  /** 弹目录选择框选新缓存父目录（取消返回 null） */
  pickCacheDir(): Promise<string | null>;
  /** 迁移缓存父目录（整体搬迁：先复制后删除；server 重启，就绪经 onServerStarted）。返回错误文案或 null */
  changeCacheDir(path: string): Promise<string | null>;
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
  checkUpdate(channel: CheckableChannel): Promise<UpdateInfo | null>;
  /** 当前更新通道偏好（主进程 config.toml 持久化；未设置回退默认值） */
  getUpdateChannel(): Promise<UpdateChannel>;
  /** 保存更新通道偏好（config.toml） */
  setUpdateChannel(channel: UpdateChannel): Promise<void>;
  /** 下载并校验上次检查到的更新（进度经 onUpdateProgress 推送；已就绪时幂等） */
  downloadUpdate(): Promise<void>;
  /** 取消进行中的下载（清半成品；无下载在跑时为空操作） */
  cancelUpdate(): Promise<void>;
  /** 重启并安装已下载的更新（成功后应用退出，不再返回） */
  installUpdate(): Promise<void>;
  /** 订阅更新包下载进度，返回退订函数 */
  onUpdateProgress(cb: (p: UpdateProgress) => void): () => void;
  /** 订阅 server 就绪（携带新地址与 token，需重配 API 并重启数据），返回退订函数。
   *  事件只发一次：页面（重）加载晚于就绪时会丢失，须与 getServerConn 拉取配合 */
  onServerStarted(cb: (conn: ServerConn) => void): () => void;
  /** 拉取当前已就绪的 server 连接（未就绪返回 null）：页面加载晚于 server 就绪的竞态兜底 */
  getServerConn(): Promise<ServerConn | null>;
  /** 订阅 server 启动/运行失败，返回退订函数 */
  onServerError(cb: (error: { message: string }) => void): () => void;
  /** 订阅 server 即将重启（旧 server 已停，应立即切启动屏），返回退订函数 */
  onServerRestarting(cb: () => void): () => void;
  /** 订阅 server 扫描进度（应用内启动屏用），返回退订函数 */
  onServerProgress(cb: (progress: ServerProgress) => void): () => void;
  /** 订阅最大化状态变化（含 Aero Snap 等系统途径），返回退订函数 */
  onWindowMaximized(cb: (maximized: boolean) => void): () => void;
}

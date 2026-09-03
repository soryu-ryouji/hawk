import type { components } from './api/schema';

// 契约类型一律从生成的 schema 取，不另写
export type Item = components['schemas']['ItemDto'];
export type FolderNode = components['schemas']['FolderNode'];
export type CategoryInfo = components['schemas']['CategoryInfo'];
export type TagInfo = components['schemas']['TagInfo'];
export type LibraryInfo = components['schemas']['LibraryInfo'];
export type ItemListRequest = components['schemas']['ItemListRequest'];
export type ItemListResult = components['schemas']['ItemListResponse'];
/** 虚拟网格骨架：全量 dim（与 item/list 同查询同排序、不分页） */
export type SkeletonItem = components['schemas']['ItemSkeletonDto'];
export type ItemSkeletonResult = components['schemas']['ItemSkeletonResponse'];

// 业务自有类型
export type ViewState =
  | { kind: 'all' }
  | { kind: 'root' }
  | { kind: 'uncategorized' }
  | { kind: 'untagged' }
  | { kind: 'folder'; path: string }
  | { kind: 'category'; name: string }
  | { kind: 'tag'; name: string }
  | { kind: 'trash' };

export interface QueryState {
  keywords: string[];
  star?: number;
  color?: string;
  orderBy: 'modification_time' | 'name' | 'size' | 'star';
  order: 'asc' | 'desc';
}

/** 视图排序偏好（.hawk/view.toml，随库同步）。scope 键：folder:<路径>（"" 为库根）/category:<名>/tag:<名> */
export type ViewPrefs = Record<string, { order_by: QueryState['orderBy']; order: 'asc' | 'desc' }>;

export interface MenuItem {
  label: string;
  danger?: boolean;
  separator?: boolean;
  /** 选中标记（排序/筛选等单选菜单的当前项） */
  checked?: boolean;
  /** 置灰不可点（如下拉中目录已被删除的历史库） */
  disabled?: boolean;
  /** 悬停提示（长路径等） */
  title?: string;
  action?: () => void;
}

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

/** Electron preload 注入的白名单通道（浏览器纯前端调试时不存在） */
declare global {
  interface Window {
    hawkShell?: {
      platform: string;
      selectLibrary(): Promise<boolean>;
      /** 本机打开过的素材库历史与当前库路径 */
      listLibraries(): Promise<{ current: string | null; libraries: LibraryHistoryItem[] }>;
      /** 打开历史素材库（仅限历史记录内的路径） */
      openLibrary(path: string): Promise<boolean>;
      /** 局域网查看设置：读取 [web] 配置与本机局域网地址 */
      /** 本机局域网 IPv4 地址列表（设置面板展示访问地址用；LAN 配置读写走 REST app/lan） */
      lanAddresses(): Promise<string[]>;
      showInFinder(relPath: string): Promise<void>;
      /** 复制库内文件的绝对路径到剪贴板 */
      copyPath(relPath: string): Promise<void>;
      /** 复制图片文件本身到剪贴板 */
      copyImage(relPath: string): Promise<void>;
      getPathForFile(file: File): string;
      minimizeWindow(): Promise<void>;
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
      /** 订阅更新包下载进度 */
      onUpdateProgress(cb: (p: UpdateProgress) => void): () => void;
      /** 订阅 server 就绪（冷启动/换库/应用设置重启都会到达，携带新地址与 token） */
      onServerStarted(cb: (conn: { address: string; token: string }) => void): () => void;
      /** 订阅 server 启动/运行失败 */
      onServerError(cb: (error: { message: string }) => void): () => void;
      /** 订阅 server 即将重启（换库/应用设置重启）：旧 server 已停，应立即切启动屏 */
      onServerRestarting(cb: () => void): () => void;
      /** 订阅 server 扫描进度（应用内启动屏用）：{ phase, processed, total }，total=0 表示不定态 */
      onServerProgress(cb: (progress: { phase: string; processed: number; total: number }) => void): () => void;
      onWindowMaximized(cb: (maximized: boolean) => void): () => void;
    };
  }
}

/** 局域网查看设置（config.toml 的 [web] 段，按库隔离） */

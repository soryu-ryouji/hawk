import type { components } from './api/schema';
import type { HawkShell, LibraryHistoryItem, UpdateInfo, UpdateProgress } from '../../electron/src/ipc-contract';

// IPC 契约类型从 electron/src/ipc-contract.ts 单一定义处 re-export（三处对齐改一处）
export type { LibraryHistoryItem, UpdateInfo, UpdateProgress };
export { UPDATE_CANCELLED } from '../../electron/src/ipc-contract';

// 契约类型一律从生成的 schema 取，不另写
export type Item = components['schemas']['ItemDto'];
export type FolderNode = components['schemas']['FolderNode'];
// 分类与标签在后端同为 TaxonInfo（扁平名字 + 计数），导出两个业务语义名
export type CategoryInfo = components['schemas']['TaxonInfo'];
export type TagInfo = components['schemas']['TaxonInfo'];
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

/** Electron preload 注入的白名单通道（形状定义在 electron/src/ipc-contract.ts；浏览器纯前端调试时不存在） */
declare global {
  interface Window {
    hawkShell?: HawkShell;
  }
}

/** 局域网查看设置（config.toml 的 [web] 段，按库隔离） */

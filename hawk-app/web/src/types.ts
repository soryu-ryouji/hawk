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

export interface MenuItem {
  label: string;
  danger?: boolean;
  separator?: boolean;
  action?: () => void;
}

/** Electron preload 注入的白名单通道（浏览器纯前端调试时不存在） */
declare global {
  interface Window {
    hawkShell?: {
      platform: string;
      selectLibrary(): Promise<boolean>;
      showInFinder(relPath: string): Promise<void>;
      getPathForFile(file: File): string;
      minimizeWindow(): Promise<void>;
      toggleMaximizeWindow(): Promise<boolean>;
      closeWindow(): Promise<void>;
      onWindowMaximized(cb: (maximized: boolean) => void): () => void;
    };
  }
}

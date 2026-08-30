// 端点封装：与 server-rest-api-v1.md 一一对应。请求/响应字段均为 snake_case（契约）。
import { apiConfig, request } from './client';
import type { components } from './schema';
import type { CategoryInfo, FolderNode, Item, ItemListRequest, ItemListResult, ItemSkeletonResult, LibraryInfo, TagInfo } from '../types';

/** server 启动状态（GET /app/startup，浏览器无 IPC 时由前端轮询） */
type StartupInfo = components['schemas']['StartupInfo'];

export interface ItemPatch {
  name?: string;
  tags?: string[];
  categories?: string[];
  star?: number;
  annotation?: string;
  url?: string;
  folder_path?: string;
}

/** 批量更新(item/batch_update):标签/分类为并集追加,评分/文件夹为设置 */
export interface ItemBatchPatch {
  add_tags?: string[];
  add_categories?: string[];
  star?: number;
  folder_path?: string;
}

export const api = {
  appInfo: () =>
    request<{ version: string; platform: string; exec_path: string; /** viewer=局域网只读查看 token */ access: 'viewer' | 'admin' }>(
      'GET',
      '/api/v1/app/info',
    ),
  startupStatus: () => request<StartupInfo>('GET', '/api/v1/app/startup'),
  libraryInfo: () => request<LibraryInfo>('GET', '/api/v1/library/info'),
  reindex: () => request<void>('POST', '/api/v1/library/reindex'),

  folderList: () => request<FolderNode>('GET', '/api/v1/folder/list'),
  folderCreate: (name: string, parentPath?: string) =>
    request<FolderNode>('POST', '/api/v1/folder/create', { body: { name, parent_path: parentPath ?? null } }),
  folderUpdate: (path: string, patch: { name?: string; parent_path?: string }) =>
    request<FolderNode>('POST', '/api/v1/folder/update', { body: { path, ...patch } }),
  folderDelete: (path: string) => request<void>('POST', '/api/v1/folder/delete', { body: { path } }),
  folderRestore: (path: string) => request<void>('POST', '/api/v1/folder/restore', { body: { path } }),

  itemList: (params: ItemListRequest) => request<ItemListResult>('POST', '/api/v1/item/list', { body: params }),
  /** 全量骨架：与 item/list 同过滤同排序（确定性次序）、不分页，只含 id/width/height/star；前端虚拟网格建完整布局用 */
  itemSkeleton: (params: Omit<ItemListRequest, 'offset' | 'limit'>) =>
    request<ItemSkeletonResult>('POST', '/api/v1/item/skeleton', { body: params }),
  itemDetail: (id: string) => request<Item>('GET', '/api/v1/item/detail', { query: { id } }),
  itemCount: () => request<number>('GET', '/api/v1/item/count'),
  itemAddByPath: (path: string, opts?: { name?: string; folder_path?: string; tags?: string[] }) =>
    request<{ item: Item; already_existed: boolean }>('POST', '/api/v1/item/add', {
      body: { path, name: opts?.name, folder_path: opts?.folder_path, tags: opts?.tags },
    }),
  // undefined 键会被 JSON.stringify 省略,即「不更新该字段」;置空传空字符串/空数组
  itemUpdate: (id: string, patch: ItemPatch, path?: string) =>
    request<Item>('POST', '/api/v1/item/update', { body: { id, path, ...patch } }),
  /** 批量更新;missing_ids 为内容不存在或移动冲突的 id(其余字段照常应用) */
  itemBatchUpdate: (ids: string[], patch: ItemBatchPatch) =>
    request<{ updated: number; missing_ids: string[] }>('POST', '/api/v1/item/batch_update', { body: { ids, ...patch } }),
  itemDelete: (id: string, path?: string) => request<void>('POST', '/api/v1/item/delete', { body: { id, path } }),
  itemRestore: (id: string, path?: string) => request<void>('POST', '/api/v1/item/restore', { body: { id, path } }),
  refreshThumbnail: (id: string) => request<void>('POST', '/api/v1/item/refresh_thumbnail', { body: { id } }),
  /**
   * 内容替换(item/replace):客户端编辑(旋转/裁切等)后的新内容提交存储层。
   * 内容哈希变化 → id 漂移,响应为新 Item(新 id),调用方应切换到新 id 继续引用。
   */
  itemReplace: (id: string, imgBase64: string, path?: string) =>
    request<Item>('POST', '/api/v1/item/replace', { body: { id, path, img_base64: imgBase64 } }),

  trashClear: () => request<void>('POST', '/api/v1/trash/clear'),

  categoryList: () => request<CategoryInfo[]>('GET', '/api/v1/category/list'),
  categoryCreate: (name: string) => request<void>('POST', '/api/v1/category/create', { body: { name } }),
  categoryUpdate: (name: string, newName: string) =>
    request<void>('POST', '/api/v1/category/update', { body: { name, new_name: newName } }),
  categoryDelete: (name: string) => request<void>('POST', '/api/v1/category/delete', { body: { name } }),

  tagList: () => request<TagInfo[]>('GET', '/api/v1/tag/list'),
  tagCreate: (name: string) => request<void>('POST', '/api/v1/tag/create', { body: { name } }),
  tagUpdate: (name: string, newName: string) =>
    request<void>('POST', '/api/v1/tag/update', { body: { name, new_name: newName } }),
  tagDelete: (name: string) => request<void>('POST', '/api/v1/tag/delete', { body: { name } }),

  /** 缩略图 URL：size 须命中服务端 thumbnail_sizes 白名单；<img> 无法带请求头，token 走查询参数（后端已放行该端点） */
  thumbnailUrl(id: string, size: number = 256): string {
    const { api: base, token } = apiConfig();
    return `${base}/api/v1/item/thumbnail?id=${encodeURIComponent(id)}&size=${size}&token=${encodeURIComponent(token)}`;
  },

  /** 原图 URL：预览浮层用；token 同样走查询参数 */
  fileUrl(id: string): string {
    const { api: base, token } = apiConfig();
    return `${base}/api/v1/item/file?id=${encodeURIComponent(id)}&token=${encodeURIComponent(token)}`;
  },
};

// 端点封装：与 server-rest-api-v1.md 一一对应。请求/响应字段均为 snake_case（契约）。
import { apiConfig, request } from './client';
import type { CategoryNode, FolderNode, Item, ItemListRequest, ItemListResult, LibraryInfo, TagInfo } from '../types';

export interface ItemPatch {
  name?: string;
  tags?: string[];
  categories?: string[];
  star?: number;
  annotation?: string;
  url?: string;
  folder_path?: string;
}

export const api = {
  appInfo: () => request<{ version: string; platform: string; exec_path: string }>('GET', '/api/v1/app/info'),
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
  itemDetail: (id: string) => request<Item>('GET', '/api/v1/item/detail', { query: { id } }),
  itemCount: () => request<number>('GET', '/api/v1/item/count'),
  itemAddByPath: (path: string, opts?: { name?: string; folder_path?: string; tags?: string[] }) =>
    request<{ item: Item; already_existed: boolean }>('POST', '/api/v1/item/add', {
      body: { path, name: opts?.name, folder_path: opts?.folder_path, tags: opts?.tags },
    }),
  // undefined 键会被 JSON.stringify 省略，即「不更新该字段」；置空传空字符串/空数组
  itemUpdate: (id: string, patch: ItemPatch, path?: string) =>
    request<Item>('POST', '/api/v1/item/update', { body: { id, path, ...patch } }),
  itemDelete: (id: string, path?: string) => request<void>('POST', '/api/v1/item/delete', { body: { id, path } }),
  itemRestore: (id: string, path?: string) => request<void>('POST', '/api/v1/item/restore', { body: { id, path } }),
  refreshThumbnail: (id: string) => request<void>('POST', '/api/v1/item/refresh_thumbnail', { body: { id } }),

  trashClear: () => request<void>('POST', '/api/v1/trash/clear'),

  categoryList: () => request<CategoryNode>('GET', '/api/v1/category/list'),
  categoryCreate: (path: string) => request<void>('POST', '/api/v1/category/create', { body: { path } }),
  categoryUpdate: (path: string, patch: { name?: string; parent_path?: string }) =>
    request<void>('POST', '/api/v1/category/update', { body: { path, ...patch } }),
  categoryDelete: (path: string) => request<void>('POST', '/api/v1/category/delete', { body: { path } }),

  tagList: () => request<TagInfo[]>('GET', '/api/v1/tag/list'),
  tagCreate: (name: string) => request<void>('POST', '/api/v1/tag/create', { body: { name } }),
  tagUpdate: (name: string, newName: string) =>
    request<void>('POST', '/api/v1/tag/update', { body: { name, new_name: newName } }),
  tagDelete: (name: string) => request<void>('POST', '/api/v1/tag/delete', { body: { name } }),

  /** 缩略图 URL：<img> 无法带请求头，token 走查询参数（后端已放行该端点） */
  thumbnailUrl(id: string, size: 256 | 1024 = 256): string {
    const { api: base, token } = apiConfig();
    return `${base}/api/v1/item/thumbnail?id=${encodeURIComponent(id)}&size=${size}&token=${encodeURIComponent(token)}`;
  },

  /** 原图 URL：预览浮层用；token 同样走查询参数 */
  fileUrl(id: string): string {
    const { api: base, token } = apiConfig();
    return `${base}/api/v1/item/file?id=${encodeURIComponent(id)}&token=${encodeURIComponent(token)}`;
  },
};

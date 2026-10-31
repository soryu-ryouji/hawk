// 端点封装：与 server-rest-api-v1.md 一一对应。请求/响应字段均为 snake_case（契约）。
import { apiConfig, request, unlockQueryValue } from './client';
import type { components } from './schema';
import type {
  CategoryInfo,
  DuplicateScanRequest,
  DuplicateScanResponse,
  FolderNode,
  GlobalFilter,
  Item,
  ItemListRequest,
  ItemListResult,
  ItemSkeletonResult,
  LibraryInfo,
  Locks,
  TagInfo,
  ViewPrefs,
} from '@/shared/types';

/** server 启动状态（GET /app/startup，浏览器无 IPC 时由前端轮询） */
type StartupInfo = components['schemas']['StartupInfo'];
export type AppInfo = Omit<components['schemas']['AppInfo'], 'role' | 'channel'> & {
  role: 'viewer' | 'importer' | 'editor' | 'admin';
  channel: 'loopback' | 'lan_web' | 'lan_peer' | 'wan';
};
export type ThumbnailSize = 512 | 1024;

export interface ItemPatch {
  name?: string;
  tags?: string[];
  categories?: string[];
  star?: number;
  annotation?: string;
  url?: string;
  folder_path?: string;
}

/** 批量更新(item/batch_update):标签/分类为并集追加,评分/文件夹为设置；
 *  paths 与 ids 等长平行（元素为 null 取主位置）：同内容多位置的选中集按位置移动 */
export interface ItemBatchPatch {
  paths?: (string | null)[];
  add_tags?: string[];
  add_categories?: string[];
  remove_tags?: string[];
  remove_categories?: string[];
  star?: number;
  folder_path?: string;
}

export const api = {
  appInfo: () => request<AppInfo>('GET', '/api/v1/app/info'),
  startupStatus: () => request<StartupInfo>('GET', '/api/v1/app/startup'),
  /** LAN Web：AccessKey 换有期 Session Cookie（图片/SSE 自动复用） */
  appSession: (accessKey: string) =>
    request<components['schemas']['AppSessionDto']>('POST', '/api/v1/app/session', {
      body: { access_key: accessKey },
    }),
  /** 局域网 web 查看配置与运行状态（admin 限定，daemon 权威读写） */
  appLan: () => request<components['schemas']['LanSettingsDto']>('GET', '/api/v1/app/lan'),
  /** 保存 [web] 配置并热重绑监听（失败 daemon 侧自动回滚并返回错误） */
  saveAppLan: (body: components['schemas']['PutLanBody']) => request<components['schemas']['LanSettingsDto']>('PUT', '/api/v1/app/lan', { body }),
  /** 目标端待确认的自动配对请求（本机 admin 设置面板轮询） */
  appLanPairRequests: () => request<components['schemas']['LanPairRequestsDto']>('GET', '/api/v1/app/lan/pair-requests'),
  appLanClients: () => request<components['schemas']['LanClientsDto']>('GET', '/api/v1/app/lan/clients'),
  createLanDeviceKey: (body: components['schemas']['LanDeviceKeyCreateBody']) =>
    request<components['schemas']['LanAccessKeyDto']>('POST', '/api/v1/app/lan/device-keys', { body }),
  patchLanDeviceKey: (keyId: string, body: components['schemas']['LanDeviceKeyUpdateBody']) =>
    request<components['schemas']['LanAccessKeyDto']>('PATCH', '/api/v1/app/lan/device-keys/' + encodeURIComponent(keyId), { body }),
  deleteLanDeviceKey: (keyId: string) =>
    request<components['schemas']['LanDeviceKeyDeletedDto']>('DELETE', '/api/v1/app/lan/device-keys/' + encodeURIComponent(keyId)),
  /** 目标端用户确认/拒绝自动配对请求并选择权限 */
  decideLanPairRequest: (requestId: string, body: { allow: boolean; write: boolean }) =>
    request<components['schemas']['LanPairDecisionDto']>('POST', `/api/v1/app/lan/pair-requests/${encodeURIComponent(requestId)}/decision`, {
      body,
    }),
  libraryInfo: () => request<LibraryInfo>('GET', '/api/v1/library/info'),
  /** 改库显示名（写库内 config.toml 的 name，daemon 热更并广播 library.updated 事件）；返回更新后的库信息 */
  libraryRename: (name: string) => request<LibraryInfo>('PATCH', '/api/v1/library/info', { body: { name } }),
  /** 周期兜底扫描设置（库级 [scan]）：保存即热生效；返回更新后的库信息 */
  libraryScanSet: (scan: { periodic: boolean; interval?: number }) => request<LibraryInfo>('PUT', '/api/v1/library/scan', { body: scan }),
  reindex: () => request<void>('POST', '/api/v1/library/reindex'),
  /** 重新扫描：强制遍历文件做复用判定（不读文件内容），收敛监听漏事件与直接改目录。
   *  path 为库内相对路径时只重扫该子树（缺省整库） */
  rescan: (path?: string) => request<void>('POST', '/api/v1/library/rescan', { body: path ? { path } : {} }),
  /** 按范围刷新派生缓存（补缺失模式）：补 0 × 0 宽高 + 缺失缩略图/调色板，不重建已有文件；
   *  附带消失对账：范围内源文件已删除但索引残留的失效位置会被移除 */
  refreshCache: (type: 'folder' | 'category' | 'tag' | 'library', value?: string) =>
    request<{ dispatched: number; removed: number }>('POST', '/api/v1/library/refresh_cache', { body: { type, value } }),
  /** 索引体检：清除不该在索引里的条目（隐藏文件 / ignore 命中 / 源文件已删），
   *  只摘索引位置不动磁盘文件；早期版本入库的残留只能靠此入口收敛 */
  cleanupIndex: () => request<{ checked: number; removed: number; hidden: number; ignored: number; missing: number }>('POST', '/api/v1/library/cleanup_index'),

  folderList: () => request<FolderNode>('GET', '/api/v1/folder/list'),
  folderCreate: (name: string, parentPath?: string) =>
    request<FolderNode>('POST', '/api/v1/folder/create', { body: { name, parent_path: parentPath ?? null } }),
  folderUpdate: (path: string, patch: { name?: string; parent_path?: string }) =>
    request<FolderNode>('POST', '/api/v1/folder/update', { body: { path, ...patch } }),
  folderDelete: (path: string) => request<void>('POST', '/api/v1/folder/delete', { body: { path } }),
  folderRestore: (path: string) => request<void>('POST', '/api/v1/folder/restore', { body: { path } }),

  itemList: (params: ItemListRequest) => request<ItemListResult>('POST', '/api/v1/item/list', { body: params }),
  /** 全量骨架：与 item/list 同过滤同排序（确定性次序）、不分页，只含 id/width/height/star；前端虚拟网格建完整布局用 */
  itemSkeleton: (params: Omit<ItemListRequest, 'offset' | 'limit'>) => request<ItemSkeletonResult>('POST', '/api/v1/item/skeleton', { body: params }),
  /** 单条详情：同内容多位置时传 path 定位具体条目（缺省主位置） */
  itemDetail: (id: string, path?: string) => request<Item>('GET', '/api/v1/item/detail', { query: path ? { id, path } : { id } }),
  itemCount: () => request<number>('GET', '/api/v1/item/count'),
  /** 重复图片分组扫描（admin）：相似 pHash / 相同内容多位置，分页按组返回 */
  itemDuplicates: (params: DuplicateScanRequest) => request<DuplicateScanResponse>('POST', '/api/v1/item/duplicates', { body: params }),
  itemAddByPath: (path: string, opts?: { name?: string; folder_path?: string; tags?: string[]; skip_existing?: boolean }) =>
    request<{ item: Item; already_existed: boolean; skipped: boolean }>('POST', '/api/v1/item/add', {
      body: { path, name: opts?.name, folder_path: opts?.folder_path, tags: opts?.tags, skip_existing: opts?.skip_existing },
    }),
  /** multipart 上传（浏览器端无文件路径，拖拽/文件选择器的内容入库）；写权限需 viewer+writable 或 admin；
   * skip_existing：内容已在库内时跳过（不写文件不追加路径），响应 skipped=true */
  itemUpload: (file: File, opts?: { folder_path?: string; name?: string; skip_existing?: boolean }) => {
    const form = new FormData();
    form.append('file', file, file.name);
    if (opts?.folder_path) form.append('folder_path', opts.folder_path);
    if (opts?.name) form.append('name', opts.name);
    if (opts?.skip_existing) form.append('skip_existing', 'true');
    return request<{ item: Item; already_existed: boolean; skipped: boolean }>('POST', '/api/v1/item/upload', { body: form });
  },
  // undefined 键会被 JSON.stringify 省略,即「不更新该字段」;置空传空字符串/空数组
  itemUpdate: (id: string, patch: ItemPatch, path?: string) => request<Item>('POST', '/api/v1/item/update', { body: { id, path, ...patch } }),
  /** 批量更新;missing_ids 为内容不存在或移动失败的 id(其余字段照常应用);
   * conflicts 为移动时因目标文件夹同名冲突而跳过的文件名 */
  itemBatchUpdate: (ids: string[], patch: ItemBatchPatch) =>
    request<{ updated: number; missing_ids: string[]; conflicts?: string[] }>('POST', '/api/v1/item/batch_update', { body: { ids, ...patch } }),
  /** 选择集共有特性聚合（标签/分类交集；多选面板数据源） */
  itemAggregate: (ids: string[]) => request<{ common_tags: string[]; common_categories: string[] }>('POST', '/api/v1/item/aggregate', { body: { ids } }),
  itemDelete: (id: string, path?: string) => request<void>('POST', '/api/v1/item/delete', { body: { id, path } }),
  itemRestore: (id: string, path?: string) => request<void>('POST', '/api/v1/item/restore', { body: { id, path } }),
  refreshThumbnail: (id: string) => request<void>('POST', '/api/v1/item/refresh_thumbnail', { body: { id } }),
  /**
   * 内容替换(item/replace):客户端编辑(旋转/裁切等)后的新内容提交存储层。
   * 内容哈希变化 → id 漂移,响应为新 Item(新 id),调用方应切换到新 id 继续引用。
   */
  itemReplace: (id: string, imgBase64: string, path?: string) => request<Item>('POST', '/api/v1/item/replace', { body: { id, path, img_base64: imgBase64 } }),

  trashClear: () => request<void>('POST', '/api/v1/trash/clear'),

  categoryList: () => request<CategoryInfo[]>('GET', '/api/v1/category/list'),
  categoryCreate: (name: string) => request<void>('POST', '/api/v1/category/create', { body: { name } }),
  categoryUpdate: (name: string, newName: string) => request<void>('POST', '/api/v1/category/update', { body: { name, new_name: newName } }),
  categoryDelete: (name: string) => request<void>('POST', '/api/v1/category/delete', { body: { name } }),

  tagList: () => request<TagInfo[]>('GET', '/api/v1/tag/list'),
  tagCreate: (name: string) => request<void>('POST', '/api/v1/tag/create', { body: { name } }),
  tagUpdate: (name: string, newName: string) => request<void>('POST', '/api/v1/tag/update', { body: { name, new_name: newName } }),
  tagDelete: (name: string) => request<void>('POST', '/api/v1/tag/delete', { body: { name } }),

  /** 全局列表隐藏项：全部素材/根目录/未分类/未标签视图的排除集（子树语义仅文件夹） */
  globalFilterList: () => request<GlobalFilter>('GET', '/api/v1/global_filter/list'),
  globalFilterSet: (kind: 'folder' | 'category' | 'tag', name: string, hidden: boolean) =>
    request<void>('PUT', '/api/v1/global_filter', { body: { kind, name, hidden } }),

  /** 锁（.hawk/locks.toml，服务端强制）：设锁/解除需 admin；解锁对任何有效 token 开放 */
  lockList: () => request<Locks>('GET', '/api/v1/lock/list'),
  lockSet: (dimension: 'folder' | 'category' | 'tag', name: string, password: string, oldPassword?: string) =>
    request<void>('POST', '/api/v1/lock/set', { body: { dimension, name, password, old_password: oldPassword } }),
  lockRemove: (dimension: 'folder' | 'category' | 'tag', name: string, password: string) =>
    request<void>('POST', '/api/v1/lock/remove', { body: { dimension, name, password } }),
  lockUnlock: (dimension: 'folder' | 'category' | 'tag', name: string, password: string) =>
    request<{ unlock_token: string }>('POST', '/api/v1/lock/unlock', { body: { dimension, name, password } }),

  /** 视图排序偏好：folder 继承由前端沿父链解析，服务端只存取原始条目 */
  viewPreferences: () => request<ViewPrefs>('GET', '/api/v1/view/preferences'),
  viewPreferenceSet: (scope: string, orderBy: 'modification_time' | 'name' | 'size' | 'star', order: 'asc' | 'desc') =>
    request<void>('PUT', '/api/v1/view/preference', { body: { scope, order_by: orderBy, order } }),
  viewPreferenceReset: (scope: string) => request<void>('DELETE', '/api/v1/view/preference', { query: { scope } }),

  /** 缩略图 URL：使用启动时换发的 Session Cookie；unlock 仅表达锁可见范围 */
  thumbnailUrl(id: string, size?: ThumbnailSize): string {
    const { api: base } = apiConfig();
    const unlock = unlockQueryValue();
    const sizeQuery = size ? `&size=${size}` : '';
    return `${base}/api/v1/item/thumbnail?id=${encodeURIComponent(id)}${sizeQuery}${unlock ? `&unlock=${encodeURIComponent(unlock)}` : ''}`;
  },

  /** 原图 URL：使用同一 Session Cookie，不携带长期 token */
  fileUrl(id: string): string {
    const { api: base } = apiConfig();
    const unlock = unlockQueryValue();
    return `${base}/api/v1/item/file?id=${encodeURIComponent(id)}${unlock ? `&unlock=${encodeURIComponent(unlock)}` : ''}`;
  },
};

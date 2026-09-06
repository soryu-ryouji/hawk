// hawk-daemon REST 客户端：Bearer 鉴权 + Envelope 解包。
// 请求一律从 background 发起（配合 host_permissions，不受页面 CORS 限制）。
//
// Token 自动发现：hawk-daemon 提供免鉴权端点 GET /api/v1/app/token，但响应不带 CORS 头
// 且 Host 限定环回地址，跨源网页 JS 读不到，只有持 host_permissions 的扩展能读——
// 因此插件无需手动填写 Token；设置里的 Token 仅作手动覆盖（如连接非本机服务）。
import { getSettings } from './settings';

interface Envelope<T> {
  status: string;
  data: T;
  /** 错误信封时存在：{code, message}（见 hawk-daemon src/api/envelope.rs） */
  error?: { code?: unknown; message?: unknown };
}

/** 从错误信封提取 “CODE：message” 形式的详情；无法解析时返回空串 */
function errorDetail(body: unknown): string {
  const err = typeof body === 'object' && body !== null ? (body as { error?: unknown }).error : null;
  if (typeof err !== 'object' || err === null) {
    return '';
  }
  const { code, message } = err as { code?: unknown; message?: unknown };
  return [typeof code === 'string' ? code : '', typeof message === 'string' ? message : '']
    .filter(Boolean)
    .join('：');
}

/** 带 HTTP 状态码的 API 错误：background 据此判断是否值得用扩展侧下载回退 */
export class HttpError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

/** 非 2xx 响应转异常：解析错误信封把服务端原因带进消息，否则通知里只有裸状态码 */
async function responseError(res: Response): Promise<Error> {
  let detail = '';
  try {
    detail = errorDetail(await res.json());
  } catch {
    // 响应体不是 JSON（网关错误页等），退回仅状态码
  }
  const message = detail ? `hawk-daemon 响应 ${res.status}（${detail}）` : `hawk-daemon 响应 ${res.status}`;
  return new HttpError(res.status, message);
}

/** 自动发现的 Token 缓存（token 每次启动随机生成，服务重启后需重新发现） */
let discovered: { serverUrl: string; token: string; at: number } | null = null;
const DISCOVERY_TTL = 60_000;

async function fetchEnvelope<T>(serverUrl: string, path: string): Promise<T> {
  const res = await fetch(`${serverUrl}${path}`);
  if (!res.ok) {
    throw await responseError(res);
  }
  const envelope = (await res.json()) as Envelope<T>;
  if (envelope.status !== 'success') {
    const detail = errorDetail(envelope);
    throw new Error(detail ? `hawk-daemon 返回错误（${detail}）` : 'hawk-daemon 返回错误');
  }
  return envelope.data;
}

async function discoverToken(serverUrl: string): Promise<string> {
  if (discovered && discovered.serverUrl === serverUrl && Date.now() - discovered.at < DISCOVERY_TTL) {
    return discovered.token;
  }
  try {
    const token = await fetchEnvelope<string>(serverUrl, '/api/v1/app/token');
    discovered = { serverUrl, token, at: Date.now() };
    return token;
  } catch (e) {
    if (e instanceof Error && e.message.includes('404')) {
      throw new Error('服务端不支持自动获取 Token，请升级 hawk 或在设置中手动填写');
    }
    throw e;
  }
}

async function request<T>(method: string, path: string, body?: unknown, retried = false): Promise<T> {
  const { serverUrl, token: manualToken } = await getSettings();
  const token = manualToken || (await discoverToken(serverUrl));
  const res = await fetch(`${serverUrl}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (res.status === 401 && !retried && !manualToken) {
    // 自动发现的 token 失效（服务重启）→ 清缓存重新发现后重试一次
    discovered = null;
    return request<T>(method, path, body, true);
  }
  if (!res.ok) {
    throw await responseError(res);
  }
  const envelope = (await res.json()) as Envelope<T>;
  if (envelope.status !== 'success') {
    const detail = errorDetail(envelope);
    throw new Error(detail ? `hawk-daemon 返回错误（${detail}）` : 'hawk-daemon 返回错误');
  }
  return envelope.data;
}

export interface LibraryInfo {
  name: string;
  path: string;
}

export interface FolderNode {
  path: string;
  name: string;
  children: FolderNode[];
}

/** 连接检查：返回素材库信息 */
export function fetchLibraryInfo() {
  return request<LibraryInfo>('GET', '/api/v1/library/info');
}

/** 文件夹树（拖拽保存面板用） */
export function fetchFolderList() {
  return request<FolderNode>('GET', '/api/v1/folder/list');
}

/** 新建文件夹（parent_path 传 null 表示根目录） */
export function createFolder(name: string) {
  return request('POST', '/api/v1/folder/create', { name, parent_path: null });
}

// 收集来源网页对应 hawk 的 Item.url 字段（与 Eagle 的 website 参数同义）：
// item/add 的 url 是下载来源（仅用于取文件），website 才是素材的「来源网址」，入库时记录到 Item.url。

/** 按图片 URL 导入（服务端负责下载）；website 为来源网页（可选） */
export function addItemByUrl(url: string, website?: string, folderPath?: string) {
  return request('POST', '/api/v1/item/add', {
    url,
    ...(website ? { website } : {}),
    ...(folderPath ? { folder_path: folderPath } : {}),
  });
}

/** 按 base64 导入（data: URL 图片）；website 为来源网页（可选） */
export function addItemByBase64(imgBase64: string, website?: string, folderPath?: string) {
  return request('POST', '/api/v1/item/add', {
    img_base64: imgBase64,
    ...(website ? { website } : {}),
    ...(folderPath ? { folder_path: folderPath } : {}),
  });
}

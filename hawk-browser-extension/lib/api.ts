// hawk 服务端 REST 客户端：本地直连 daemon / 经 hawk-server 域名两套连接方式。
// 请求一律从 background 发起（配合 host_permissions，不受页面 CORS 限制）。
//
// 本地模式：Bearer token 自动发现（GET /api/v1/app/token 仅环回地址可读）。
// 远程模式：用 hawk-server 写入密钥登录查看面，换取写作用域令牌，打开目标设备会话，
// 之后的素材请求经查看面代理转发到目标 daemon。
import { getSettings, saveSettings, type HawkSettings } from './settings';

interface Envelope<T> {
  status: string;
  data: T;
  /** 错误信封时存在：{code, message} */
  error?: { code?: unknown; message?: unknown };
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

/** hawk-server 设备列表（查看面结构，与 daemon RemoteDeviceDto 对齐） */
export interface RemoteDevice {
  device_id: string;
  name: string;
  platform: string;
  library_name: string;
  online: boolean;
  allow_remote: boolean;
  allow_write: boolean;
  last_seen_at_ms?: number;
}

interface ViewerSessionInfo {
  session_token: string;
  access: 'read' | 'write';
  target_device_id: string;
  target_name: string;
  target_library_name: string;
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

/** 带 HTTP 状态码的 API 错误：调用方据此判断是否重登/重试 */
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
  const message = detail ? `服务端响应 ${res.status}（${detail}）` : `服务端响应 ${res.status}`;
  return new HttpError(res.status, message);
}

interface CallOptions {
  method: string;
  token?: string;
  body?: unknown;
}

/** 单次信封请求：不重试，token 由调用方提供 */
async function call<T>(base: string, path: string, opts: CallOptions): Promise<T> {
  const res = await fetch(`${base}${path}`, {
    method: opts.method,
    headers: {
      ...(opts.token ? { Authorization: `Bearer ${opts.token}` } : {}),
      ...(opts.body !== undefined ? { 'Content-Type': 'application/json' } : {}),
    },
    body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
  });
  if (!res.ok) {
    throw await responseError(res);
  }
  const envelope = (await res.json()) as Envelope<T>;
  if (envelope.status !== 'success') {
    const detail = errorDetail(envelope);
    throw new Error(detail ? `服务端返回错误（${detail}）` : '服务端返回错误');
  }
  return envelope.data;
}

/** 自动发现的 Token 缓存（token 每次启动随机生成，服务重启后需重新发现） */
let discovered: { serverUrl: string; token: string; at: number } | null = null;
const DISCOVERY_TTL = 60_000;

async function discoverToken(serverUrl: string): Promise<string> {
  if (discovered && discovered.serverUrl === serverUrl && Date.now() - discovered.at < DISCOVERY_TTL) {
    return discovered.token;
  }
  try {
    const token = await call<string>(serverUrl, '/api/v1/app/token', { method: 'GET' });
    discovered = { serverUrl, token, at: Date.now() };
    return token;
  } catch (e) {
    if (e instanceof Error && e.message.includes('404')) {
      throw new Error('服务端不支持自动获取 Token，请升级 hawk 或在设置中手动填写');
    }
    throw e;
  }
}

async function localContext(): Promise<{ base: string; token: string }> {
  const s = await getSettings();
  const token = s.localToken || (await discoverToken(s.localUrl));
  return { base: s.localUrl, token };
}

async function remoteLogin(settings: HawkSettings): Promise<string> {
  let data: { token: string; access: string };
  try {
    data = await call<{ token: string; access: string }>(
      settings.remoteUrl,
      '/api/v1/viewer/login',
      { method: 'POST', body: { key: settings.remoteKey } },
    );
  } catch (e) {
    if (e instanceof HttpError && e.status === 401) {
      throw new Error('hawk-server 写入密钥无效');
    }
    throw e;
  }
  if (data.access !== 'write') {
    throw new Error('该密钥只有浏览权限，请在插件设置中填写 hawk-server 写入密钥');
  }
  return data.token;
}

/** 确保当前令牌已绑定到目标素材库的写会话 */
async function ensureRemoteSession(
  settings: HawkSettings,
  token: string,
): Promise<void> {
  const current = await call<ViewerSessionInfo | null>(
    settings.remoteUrl,
    '/api/v1/viewer/session',
    { method: 'GET', token },
  );
  if (
    current &&
    current.access === 'write' &&
    current.target_device_id === settings.targetDeviceId
  ) {
    return;
  }
  await call<ViewerSessionInfo>(settings.remoteUrl, '/api/v1/viewer/session', {
    method: 'POST',
    token,
    body: { target_device_id: settings.targetDeviceId, access: 'write' },
  });
}

/**
 * 远程连接上下文：优先复用缓存的写令牌；401 时重登一次。
 * forceLogin = true 时忽略缓存令牌（调用方已确认缓存失效）。
 */
async function remoteContext(forceLogin = false): Promise<{ base: string; token: string }> {
  const settings = await getSettings();
  if (!settings.remoteUrl) {
    throw new Error('请先在插件设置中填写 hawk-server 域名');
  }
  if (!settings.remoteKey) {
    throw new Error('请先在插件设置中填写 hawk-server 写入密钥');
  }
  if (!settings.targetDeviceId) {
    throw new Error('请先在插件设置中选择要保存到的素材库');
  }

  let token = forceLogin ? '' : settings.remoteToken;
  if (!token) {
    token = await remoteLogin(settings);
    await saveSettings({ remoteToken: token });
  }

  try {
    await ensureRemoteSession(settings, token);
    return { base: settings.remoteUrl, token };
  } catch (e) {
    if (!(e instanceof HttpError) || e.status !== 401 || forceLogin) {
      throw e;
    }
    // 缓存的令牌已失效（服务端数据重置/注销）：重登后重开会话
    const fresh = await remoteLogin(settings);
    await saveSettings({ remoteToken: fresh });
    await ensureRemoteSession(settings, fresh);
    return { base: settings.remoteUrl, token: fresh };
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const settings = await getSettings();
  if (settings.mode === 'remote') {
    const { base, token } = await remoteContext();
    try {
      return await call<T>(base, path, { method, token, body });
    } catch (e) {
      if (!(e instanceof HttpError) || e.status !== 401) {
        throw e;
      }
      const fresh = await remoteContext(true);
      return call<T>(fresh.base, path, { method, token: fresh.token, body });
    }
  }

  const { base, token } = await localContext();
  try {
    return await call<T>(base, path, { method, token, body });
  } catch (e) {
    // 自动发现的 token 失效（服务重启）→ 清缓存重新发现后重试一次
    if (e instanceof HttpError && e.status === 401 && !settings.localToken) {
      discovered = null;
      const retry = await localContext();
      return call<T>(retry.base, path, { method, token: retry.token, body });
    }
    throw e;
  }
}

/** 连接检查：返回素材库信息（远程模式会确保目标会话已建立） */
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
// 图片内容由扩展下载后以 base64 提交，website 即素材的「来源网址」，入库时记录到 Item.url。

/** 按 base64 导入（浏览器下载后的图片内容）；website 为来源网页（可选） */
export function addItemByBase64(imgBase64: string, website?: string, folderPath?: string) {
  return request('POST', '/api/v1/item/add', {
    img_base64: imgBase64,
    ...(website ? { website } : {}),
    ...(folderPath ? { folder_path: folderPath } : {}),
  });
}

/**
 * 远程模式：用写入密钥登录并列出可写素材库。
 * 不依赖已保存设置，供设置弹窗在保存前预览设备列表。
 */
export async function listRemoteDevices(remoteUrl: string, remoteKey: string): Promise<RemoteDevice[]> {
  if (!remoteUrl.trim()) {
    throw new Error('请填写 hawk-server 域名');
  }
  if (!remoteKey.trim()) {
    throw new Error('请填写 hawk-server 写入密钥');
  }
  const base = remoteUrl.trim().replace(/\/+$/, '');
  const key = remoteKey.trim();
  const settings = await getSettings();
  const reusable = settings.remoteUrl === base && settings.remoteKey === key ? settings.remoteToken : '';
  if (reusable) {
    try {
      const data = await call<{ devices: RemoteDevice[] }>(base, '/api/v1/viewer/devices', {
        method: 'GET',
        token: reusable,
      });
      return data.devices;
    } catch (e) {
      if (!(e instanceof HttpError) || e.status !== 401) {
        throw e;
      }
    }
  }
  const token = await remoteLogin({ ...settings, remoteUrl: base, remoteKey: key });
  if (settings.remoteUrl === base && settings.remoteKey === key) {
    await saveSettings({ remoteToken: token });
  }
  const data = await call<{ devices: RemoteDevice[] }>(base, '/api/v1/viewer/devices', {
    method: 'GET',
    token,
  });
  return data.devices;
}

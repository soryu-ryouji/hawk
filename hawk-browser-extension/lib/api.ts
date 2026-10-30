// hawk 服务端 REST 客户端：本地直连 daemon / 经 hawk-server 域名两套连接方式。
// 请求一律从 background 发起（配合 host_permissions，不受页面 CORS 限制）。
//
// 本地模式：Bearer token 自动发现（GET /api/v1/app/admin-token 仅环回地址可读）。
// 远程模式：两级验证——service-key（池门禁）换门令牌，门令牌 + hawk-app key（须带写权限）
// 换写作用域令牌（bt 绑定唯一目标素材库），之后的素材请求经查看面代理转发到目标 daemon。
import { getSettings, saveSettings, type HawkSettings } from './settings';
import { discoverLocalBase, resetLocalBase } from './local-discovery';

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
    const token = await call<string>(serverUrl, '/api/v1/app/admin-token', { method: 'GET' });
    discovered = { serverUrl, token, at: Date.now() };
    return token;
  } catch (e) {
    if (e instanceof Error && e.message.includes('404')) {
      throw new Error('服务端不支持自动获取 Token，请升级 hawk 或在设置中手动填写');
    }
    throw e;
  }
}

/** 当前目标：本地库，或本地 hawk 正在浏览的远端库（跟随它的会话代理） */
export interface ResolvedTarget {
  kind: 'local' | 'remote';
  base: string;
  token: string;
  writable: boolean;
  label: string;
}

/** daemon 的当前连接描述符（GET /api/v1/app/connection，admin 鉴权） */
interface AppConnection {
  mode: 'local' | 'remote' | 'none';
  api_base?: string;
  credential?: string;
  device_id?: string;
  label?: string;
  writable: boolean;
}

/**
 * 本地模式的目标解析：跟随本地 hawk 当前浏览的库——
 * 本地库直连本地 daemon；正在浏览远端库时用会话代理地址 + 会话令牌（与 app 渲染进程同一条通道，
 * 远端是否可写由目标设备的「允许广域网保存素材」决定，app/connection 的 writable 即该结论）。
 * 本地 daemon 的端口由 app 决定（默认 27371，自动选择限 27371–27400），插件按同一段探活；
 * 高级选项里的端口覆盖优先（见 local-discovery.ts）。
 */
export async function resolveLocalTarget(): Promise<ResolvedTarget> {
  const s = await getSettings();
  const base = await discoverLocalBase(parsePort(s.localPort));
  const adminToken = s.localToken || (await discoverToken(base));
  const conn = await call<AppConnection>(base, '/api/v1/app/connection', {
    method: 'GET',
    token: adminToken,
  });
  if (conn.mode === 'remote' && conn.api_base && conn.credential) {
    return {
      kind: 'remote',
      base: conn.api_base,
      token: conn.credential,
      writable: conn.writable,
      label: conn.label ?? '',
    };
  }
  if (conn.mode !== 'local') {
    throw new Error('本地 hawk 未连接任何素材库');
  }
  return { kind: 'local', base, token: adminToken, writable: true, label: '' };
}

/** 高级选项里的端口（留空/非法 → 不用覆盖，走自动发现） */
function parsePort(value: string): number | undefined {
  const port = Number(value.trim());
  return value.trim() !== '' && Number.isInteger(port) && port >= 1 && port <= 65535 ? port : undefined;
}

interface RemoteLoginResult {
  token: string;
  /** bt 绑定的目标设备（hawk-app key 映射决定，无需用户再选） */
  deviceId: string;
}

/** 两级登录：service-key → 门令牌（5min/5 次尝试）；门令牌 + hawk-app key → bt */
async function remoteLogin(settings: HawkSettings): Promise<RemoteLoginResult> {
  let gate: { gate_token: string };
  try {
    gate = await call<{ gate_token: string }>(settings.remoteUrl, '/api/v1/viewer/gate', {
      method: 'POST',
      body: { service_key: settings.serviceKey },
    });
  } catch (e) {
    if (e instanceof HttpError && e.status === 401) {
      throw new Error('服务密钥（service-key）无效');
    }
    throw e;
  }
  let data: { token: string; access: string; device_id: string };
  try {
    data = await call<{ token: string; access: string; device_id: string }>(
      settings.remoteUrl,
      '/api/v1/viewer/login',
      { method: 'POST', body: { gate_token: gate.gate_token, key: settings.remoteKey } },
    );
  } catch (e) {
    if (e instanceof HttpError && e.status === 401) {
      throw new Error('访问密钥无效或已吊销');
    }
    throw e;
  }
  if (data.access !== 'write') {
    throw new Error('该访问密钥没有写权限，请在 hawk 设置 → 远程中为该密钥开启写权限');
  }
  return { token: data.token, deviceId: data.device_id };
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
  if (!settings.serviceKey) {
    throw new Error('请先在插件设置中填写服务密钥（service-key）');
  }
  if (!settings.remoteKey) {
    throw new Error('请先在插件设置中填写访问密钥（须带写权限）');
  }

  let token = forceLogin ? '' : settings.remoteToken;
  if (!token) {
    const login = await remoteLogin(settings);
    token = login.token;
    // bt 绑定的目标设备即素材库（访问密钥映射决定），同步进设置免再选
    await saveSettings({ remoteToken: token, targetDeviceId: login.deviceId });
    settings.targetDeviceId = login.deviceId;
  }
  if (!settings.targetDeviceId) {
    // 有缓存令牌但无目标记录（旧版残留）：重登一次拿绑定设备
    const login = await remoteLogin(settings);
    token = login.token;
    await saveSettings({ remoteToken: token, targetDeviceId: login.deviceId });
    settings.targetDeviceId = login.deviceId;
  }

  try {
    await ensureRemoteSession(settings, token);
    return { base: settings.remoteUrl, token };
  } catch (e) {
    if (!(e instanceof HttpError) || e.status !== 401 || forceLogin) {
      throw e;
    }
    // 缓存的令牌已失效（服务端数据重置/注销）：重登后重开会话
    const login = await remoteLogin(settings);
    await saveSettings({ remoteToken: login.token, targetDeviceId: login.deviceId });
    settings.targetDeviceId = login.deviceId;
    await ensureRemoteSession(settings, login.token);
    return { base: settings.remoteUrl, token: login.token };
  }
}

interface RequestOptions {
  /** 写请求：本地 hawk 正在浏览一个不可写的远端库时提前给出可读错误（否则只是裸 403） */
  write?: boolean;
}

async function request<T>(
  method: string,
  path: string,
  body?: unknown,
  opts: RequestOptions = {},
): Promise<T> {
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

  const target = await resolveLocalTarget();
  if (opts.write && !target.writable) {
    throw new Error(
      `本地 hawk 正在浏览的远端库「${target.label || target.base}」未开启远程写入` +
        '（在目标设备 hawk 设置 → 远程 → 允许广域网保存素材 打开）',
    );
  }
  try {
    return await call<T>(target.base, path, { method, token: target.token, body });
  } catch (e) {
    // 自动发现的 token 失效（app 重启）或远端会话已换（重连后新代理端口）→ 重新解析重试一次
    if (e instanceof HttpError && e.status === 401) {
      discovered = null;
      resetLocalBase(); // app 重启可能换了端口，重试时重新探活
      const retry = await resolveLocalTarget();
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
  return request('POST', '/api/v1/folder/create', { name, parent_path: null }, { write: true });
}

// 收集来源网页对应 hawk 的 Item.url 字段（与 Eagle 的 website 参数同义）：
// 图片内容由扩展下载后以 base64 提交，website 即素材的「来源网址」，入库时记录到 Item.url。

/** 按 base64 导入（浏览器下载后的图片内容）；website 为来源网页（可选） */
export function addItemByBase64(imgBase64: string, website?: string, folderPath?: string) {
  return request(
    'POST',
    '/api/v1/item/add',
    {
      img_base64: imgBase64,
      ...(website ? { website } : {}),
      ...(folderPath ? { folder_path: folderPath } : {}),
    },
    { write: true },
  );
}

/**
 * 远程模式：两级登录并列出访问密钥绑定的素材库（新模型下恒为单个——bt 绑定唯一设备）。
 * 不依赖已保存设置，供设置弹窗在保存前预览。
 */
export async function listRemoteDevices(
  remoteUrl: string,
  serviceKey: string,
  remoteKey: string,
): Promise<RemoteDevice[]> {
  if (!remoteUrl.trim()) {
    throw new Error('请填写 hawk-server 域名');
  }
  if (!serviceKey.trim()) {
    throw new Error('请填写服务密钥（service-key）');
  }
  if (!remoteKey.trim()) {
    throw new Error('请填写访问密钥（须带写权限）');
  }
  const base = remoteUrl.trim().replace(/\/+$/, '');
  const sk = serviceKey.trim();
  const key = remoteKey.trim();
  const settings = await getSettings();
  const reusable =
    settings.remoteUrl === base && settings.serviceKey === sk && settings.remoteKey === key
      ? settings.remoteToken
      : '';
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
  const login = await remoteLogin({ ...settings, remoteUrl: base, serviceKey: sk, remoteKey: key });
  if (settings.remoteUrl === base && settings.serviceKey === sk && settings.remoteKey === key) {
    await saveSettings({ remoteToken: login.token, targetDeviceId: login.deviceId });
  }
  const data = await call<{ devices: RemoteDevice[] }>(base, '/api/v1/viewer/devices', {
    method: 'GET',
    token: login.token,
  });
  return data.devices;
}

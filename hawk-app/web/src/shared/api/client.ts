// API 连接配置与请求封装：baseURL、Bearer 头、信封解包、ApiError。
// 连接参数经 URL hash 注入（Electron 主进程），或开发时用 VITE_HAWK_API/TOKEN 环境变量。

import { hasShell } from '@/shared/lib/platform';
import { loadText, removeKey, saveText, STORAGE_KEYS } from '@/shared/lib/persist';

interface ApiConfig {
  api: string;
  token: string;
}

let config: ApiConfig | null = null;

export class ApiError extends Error {
  constructor(
    public code: string,
    message: string,
    public httpStatus: number,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

/** 解析连接参数；都缺时返回 null（启动失败态）。浏览器直连 hawk-daemon（局域网 web 查看）时无显式参数，回退同源 */
export function initApi(): ApiConfig | null {
  const hash = new URLSearchParams(location.hash.replace(/^#/, ''));
  const search = new URLSearchParams(location.search);
  // Electron 壳必须经 hash 注入（dev 也可用 VITE_HAWK_API）；纯浏览器则假定页面由 hawk-daemon 托管（同源）
  const api = hash.get('api') || (import.meta.env.VITE_HAWK_API as string | undefined) || (!hasShell ? location.origin : null);
  if (!api) {
    config = null;
    return null;
  }
  // token 优先级：hash（Electron 注入）> ?token= 查询参数 > 本地存储（按 api 地址隔离，记住上次验证通过的 token）
  const token = hash.get('token') || search.get('token') || loadText(tokenStorageKey(api)) || '';
  config = { api, token };
  return config;
}

/** token 在 localStorage 的存储键：按 api host 隔离，多素材库/多服务端互不覆盖 */
export function tokenStorageKey(api: string): string {
  return STORAGE_KEYS.token(new URL(api).host);
}

/** 记住验证通过的 token（局域网查看器下次免输入直连） */
export function storeToken(api: string, token: string): void {
  saveText(tokenStorageKey(api), token);
}

export function clearStoredToken(api: string): void {
  removeKey(tokenStorageKey(api));
}

/** 更新当前连接 token（ConnectScreen 验证通过后注入） */
export function setApiToken(token: string): void {
  apiConfig().token = token;
}

// ---- 文件夹/分类/标签锁的解锁票据（内存态；daemon 重启即失效，重新输密码即可） ----
// 票据由各客户端独立持有：分享页面链接不带票据，解锁状态不扩散

/** 已解锁的锁条目：key = `${dimension}:${name}`（name 为锁条目名，文件夹是覆盖该路径的最近锁定祖先） */
const unlockTickets = new Map<string, string>();

export function lockKey(dimension: 'folder' | 'category' | 'tag', name: string): string {
  return `${dimension}:${name}`;
}

export function hasUnlockTicket(dimension: 'folder' | 'category' | 'tag', name: string): boolean {
  return unlockTickets.has(lockKey(dimension, name));
}

/** 解锁成功后登记票据：后续请求自动附带（header + img/SSE 查询参数） */
export function addUnlockTicket(dimension: 'folder' | 'category' | 'tag', name: string, ticket: string): void {
  unlockTickets.set(lockKey(dimension, name), ticket);
}

/** 丢弃票据即「锁定回去」（仅影响本客户端；票据仍留在 daemon 内存，但无持有者） */
export function dropUnlockTicket(dimension: 'folder' | 'category' | 'tag', name: string): void {
  unlockTickets.delete(lockKey(dimension, name));
}

/** 清空全部票据（换连接/测试用） */
export function clearUnlockTickets(): void {
  unlockTickets.clear();
}

/** 全部票据的请求头值（逗号拼接；无票据返回 null） */
export function unlockHeaderValue(): string | null {
  return unlockTickets.size > 0 ? [...unlockTickets.values()].join(',') : null;
}

/** 全部票据的查询参数值（<img> 直链与 SSE 用；无票据返回 null） */
export function unlockQueryValue(): string | null {
  return unlockTickets.size > 0 ? [...unlockTickets.values()].join(',') : null;
}

/** server 重启后整体更换连接（主进程经 hawk:server-started 推送新地址/token）；
 *  票据是 daemon 内存态，重启即失效 → 同步清空 */
export function configureApi(next: { api: string; token: string }): void {
  config = { ...next };
  clearUnlockTickets();
}

export function apiConfig(): ApiConfig {
  if (!config) {
    throw new ApiError('NO_CONFIG', '缺少后端连接配置', 0);
  }
  return config;
}

/** 统一请求：信封解包，status==='error' 或 HTTP 非 2xx 时抛 ApiError */
export async function request<T>(method: string, path: string, opts?: { body?: unknown; query?: Record<string, string> }): Promise<T> {
  const { api, token } = apiConfig();
  const url = new URL(api + path);
  for (const [key, value] of Object.entries(opts?.query ?? {})) {
    url.searchParams.set(key, value);
  }

  // FormData（multipart 上传）不做 JSON 序列化，让 fetch 自带 boundary 的 Content-Type
  const raw = opts?.body instanceof FormData ? opts.body : undefined;
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        ...(unlockHeaderValue() ? { 'X-Hawk-Unlock': unlockHeaderValue()! } : {}),
        ...(opts?.body !== undefined && !raw ? { 'Content-Type': 'application/json' } : {}),
      },
      body: raw ?? (opts?.body !== undefined ? JSON.stringify(opts.body) : undefined),
    });
  } catch {
    throw new ApiError('NETWORK', '无法连接 hawk-daemon', 0);
  }

  const envelope = (await res.json().catch(() => null)) as {
    status: string;
    data?: T;
    error?: { code: string; message: string };
  } | null;

  if (!res.ok || !envelope || envelope.status === 'error') {
    throw new ApiError(envelope?.error?.code ?? 'INTERNAL', envelope?.error?.message ?? `HTTP ${res.status}`, res.status);
  }
  return envelope.data as T;
}

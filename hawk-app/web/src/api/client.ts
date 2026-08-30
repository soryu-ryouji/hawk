// API 连接配置与请求封装：baseURL、Bearer 头、信封解包、ApiError。
// 连接参数经 URL hash 注入（Electron 主进程），或开发时用 VITE_HAWK_API/TOKEN 环境变量。

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

/** 解析连接参数；都缺时返回 null（启动失败态）。浏览器直连 hawk-server（局域网 web 查看）时无显式参数，回退同源 */
export function initApi(): ApiConfig | null {
  const hash = new URLSearchParams(location.hash.replace(/^#/, ''));
  const search = new URLSearchParams(location.search);
  // Electron 壳必须经 hash 注入（dev 也可用 VITE_HAWK_API）；纯浏览器则假定页面由 hawk-server 托管（同源）
  const api =
    hash.get('api') ||
    (import.meta.env.VITE_HAWK_API as string | undefined) ||
    (!window.hawkShell ? location.origin : null);
  if (!api) {
    config = null;
    return null;
  }
  // token 优先级：hash（Electron 注入）> ?token= 查询参数 > 本地存储（按 api 地址隔离，记住上次验证通过的 token）
  const token =
    hash.get('token') || search.get('token') || localStorage.getItem(tokenStorageKey(api)) || '';
  config = { api, token };
  return config;
}

/** token 在 localStorage 的存储键：按 api host 隔离，多素材库/多服务端互不覆盖 */
export function tokenStorageKey(api: string): string {
  return `hawk:token:${new URL(api).host}`;
}

/** 记住验证通过的 token（局域网查看器下次免输入直连） */
export function storeToken(api: string, token: string): void {
  localStorage.setItem(tokenStorageKey(api), token);
}

export function clearStoredToken(api: string): void {
  localStorage.removeItem(tokenStorageKey(api));
}

/** 更新当前连接 token（ConnectScreen 验证通过后注入） */
export function setApiToken(token: string): void {
  apiConfig().token = token;
}

export function apiConfig(): ApiConfig {
  if (!config) {
    throw new ApiError('NO_CONFIG', '缺少后端连接配置', 0);
  }
  return config;
}

/** 统一请求：信封解包，status==='error' 或 HTTP 非 2xx 时抛 ApiError */
export async function request<T>(
  method: string,
  path: string,
  opts?: { body?: unknown; query?: Record<string, string> },
): Promise<T> {
  const { api, token } = apiConfig();
  const url = new URL(api + path);
  for (const [key, value] of Object.entries(opts?.query ?? {})) {
    url.searchParams.set(key, value);
  }

  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        ...(opts?.body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      },
      body: opts?.body !== undefined ? JSON.stringify(opts.body) : undefined,
    });
  } catch {
    throw new ApiError('NETWORK', '无法连接 hawk-server', 0);
  }

  const envelope = (await res.json().catch(() => null)) as {
    status: string;
    data?: T;
    error?: { code: string; message: string };
  } | null;

  if (!res.ok || !envelope || envelope.status === 'error') {
    throw new ApiError(
      envelope?.error?.code ?? 'INTERNAL',
      envelope?.error?.message ?? `HTTP ${res.status}`,
      res.status,
    );
  }
  return envelope.data as T;
}

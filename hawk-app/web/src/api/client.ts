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

/** 解析连接参数；都缺时返回 null（启动失败态） */
export function initApi(): ApiConfig | null {
  const hash = new URLSearchParams(location.hash.replace(/^#/, ''));
  const api = hash.get('api') || (import.meta.env.VITE_HAWK_API as string | undefined);
  const token = hash.get('token') || (import.meta.env.VITE_HAWK_TOKEN as string | undefined);
  config = api && token ? { api, token } : null;
  return config;
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

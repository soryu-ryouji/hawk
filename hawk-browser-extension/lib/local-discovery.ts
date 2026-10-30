// 本地 daemon 的地址发现：端口由 hawk app 配置（默认 27371），app 的「自动选择可用端口」
// 只在 27371–27400 段内挑，插件按同一段顺序探活即零配置接入。
// 契约跨仓库：hawk-app/electron/src/app-config.ts 的 API_PORT_RANGE（app 填入段外端口时，
// 用户需在本插件的高级选项里指定端口，覆盖端口优先于自动发现）。
//
// 探活打的是 `GET /api/v1/app/admin-token`：免鉴权、响应不带 CORS 头（跨源网页 JS 读不到，持
// host_permissions 的扩展可读），本就是为生态客户端零配置接入设计的端点。
const PORT_RANGE: readonly [number, number] = [27371, 27400];
/** 命中结果缓存时长：app 重启/换端口后最迟这么久自动跟上（401 路径会立即作废缓存） */
const CACHE_TTL = 60_000;

let cached: { base: string; at: number } | null = null;

/** 作废发现缓存（app 重启、端口变更、401 重试时调用） */
export function resetLocalBase(): void {
  cached = null;
}

/** 本地 daemon 的 base URL：overridePort（高级选项）优先，否则在候选段内探活 */
export async function discoverLocalBase(overridePort?: number): Promise<string> {
  if (overridePort) {
    return `http://127.0.0.1:${overridePort}`;
  }
  if (cached && Date.now() - cached.at < CACHE_TTL) {
    return cached.base;
  }
  for (let port = PORT_RANGE[0]; port <= PORT_RANGE[1]; port += 1) {
    const base = `http://127.0.0.1:${port}`;
    if (await probe(base)) {
      cached = { base, at: Date.now() };
      return base;
    }
  }
  throw new Error(
    `未找到本机 hawk（已探测 127.0.0.1:${PORT_RANGE[0]}–${PORT_RANGE[1]}）。` +
      '请先在 hawk 中打开素材库或远程查看；若在 hawk 设置 → 网络 → 本地服务里把端口改到了这段之外，' +
      '请把端口填到本插件的「高级选项」中',
  );
}

/** 探活：能应答 token 发现端点即认定该端口是 hawk-daemon */
async function probe(base: string): Promise<boolean> {
  try {
    const res = await fetch(`${base}/api/v1/app/admin-token`, { signal: AbortSignal.timeout(500) });
    if (!res.ok) {
      return false;
    }
    const body = (await res.json()) as { status?: unknown; data?: unknown };
    return body.status === 'success' && typeof body.data === 'string';
  } catch {
    return false; // 端口未监听/超时：继续下一个
  }
}

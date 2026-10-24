// iOS 配对二维码载荷（与 docs/frontend/ios-app.md「鉴权与配对」及 hawk-ios 的 PairingPayload 对齐）。
// 编码产物必须是 snake_case 字段名；解码端按原始 JSON 键读取，不做 camelCase 转换。

/** 配对载荷版本：解码端遇到更高版本应提示升级，而不是静默失败 */
export const PAIRING_PAYLOAD_VERSION = 1;

export type PairingKind = 'lan' | 'remote';

export interface LanPairingPayload {
  v: typeof PAIRING_PAYLOAD_VERSION;
  kind: 'lan';
  /** 首个候选地址（旧客户端兼容字段） */
  url: string;
  /** 同一目标的所有可用候选地址，由客户端依次探测 */
  urls: string[];
  token: string;
}

export interface RemotePairingPayload {
  v: typeof PAIRING_PAYLOAD_VERSION;
  kind: 'remote';
  url: string;
  service_key: string;
  key: string;
}

export type PairingPayload = LanPairingPayload | RemotePairingPayload;

/** 局域网：urls 为同一目标的所有候选地址（http://ip:port），token 为访问密钥 */
export function lanPairingPayload(urls: string[], token: string): LanPairingPayload {
  const candidates = urls.map((url) => url.trim()).filter(Boolean);
  return { v: PAIRING_PAYLOAD_VERSION, kind: 'lan', url: candidates[0] ?? '', urls: candidates, token };
}

/** 广域网：url 为 hawk-server 地址，key 为本机新签发的只读 hawk-app key */
export function remotePairingPayload(url: string, serviceKey: string, key: string): RemotePairingPayload {
  return { v: PAIRING_PAYLOAD_VERSION, kind: 'remote', url, service_key: serviceKey, key };
}

/** 二维码承载的字符串（JSON；字段名即协议字段名） */
export function encodePairingPayload(payload: PairingPayload): string {
  return JSON.stringify(payload);
}

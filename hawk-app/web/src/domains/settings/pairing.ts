// 移动端配对二维码载荷（iOS / Android 共用协议；字段与两端 PairingPayload 对齐）。
// 纯局域网：只包含目标地址候选与访问密钥。

export const PAIRING_PAYLOAD_VERSION = 3;

export interface PairingPayload {
  v: typeof PAIRING_PAYLOAD_VERSION;
  /** 目标库的稳定标识 */
  target_library_id?: string;
  /** 目标 daemon 签发的访问密钥 */
  access_key: string;
  /** 同一目标的局域网候选地址，客户端自动择优 */
  lan_endpoints: string[];
}

export function lanPairingPayload(urls: string[], accessKey: string, targetLibraryId?: string): PairingPayload {
  return {
    v: PAIRING_PAYLOAD_VERSION,
    target_library_id: targetLibraryId,
    access_key: accessKey,
    lan_endpoints: urls.map((url) => url.trim()).filter(Boolean),
  };
}

export function encodePairingPayload(payload: PairingPayload): string {
  return JSON.stringify(payload);
}

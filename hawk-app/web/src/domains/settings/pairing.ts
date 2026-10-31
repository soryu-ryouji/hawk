// 移动端配对二维码载荷（iOS / Android 共用协议；字段与两端 PairingPayload 对齐）。
// 同一 AccessKey + target_identity 可跨 LAN/WAN 使用，不再分发 tenant/tenant key。

export const PAIRING_PAYLOAD_VERSION = 2;

export interface PairingPayload {
  v: typeof PAIRING_PAYLOAD_VERSION;
  /** 目标库的稳定标识；无本地库的远程查看模式可为空 */
  target_library_id?: string;
  /** WAN rendezvous 所需；仅 LAN 配对可为空 */
  target_device_id?: string;
  /** 配对时带外钉扎的目标 iroh 公钥 */
  target_identity: string;
  /** LAN Web 自签证书指纹；原生 iroh 不使用 */
  tls_fingerprint?: string;
  /** 只在客户端与目标 daemon 的加密通道内出示 */
  access_key: string;
  /** 同一目标的 LAN 候选地址，客户端自动择优 */
  lan_endpoints: string[];
  wan?: {
    server_url: string;
  };
}

export function lanPairingPayload(
  urls: string[],
  accessKey: string,
  targetIdentity: string,
  targetLibraryId?: string,
  targetDeviceId?: string,
  tlsFingerprint?: string,
): PairingPayload {
  return {
    v: PAIRING_PAYLOAD_VERSION,
    target_library_id: targetLibraryId,
    target_device_id: targetDeviceId,
    target_identity: targetIdentity,
    tls_fingerprint: tlsFingerprint,
    access_key: accessKey,
    lan_endpoints: urls.map((url) => url.trim()).filter(Boolean),
  };
}

export function remotePairingPayload(
  serverUrl: string,
  accessKey: string,
  targetIdentity: string,
  targetDeviceId: string,
  targetLibraryId?: string,
  lanEndpoints: string[] = [],
): PairingPayload {
  return {
    v: PAIRING_PAYLOAD_VERSION,
    target_library_id: targetLibraryId,
    target_device_id: targetDeviceId,
    target_identity: targetIdentity,
    access_key: accessKey,
    lan_endpoints: lanEndpoints,
    wan: { server_url: serverUrl.trim().replace(/\/+$/, '') },
  };
}

export function encodePairingPayload(payload: PairingPayload): string {
  return JSON.stringify(payload);
}

import { describe, expect, it } from 'vitest';
import { encodePairingPayload, lanPairingPayload, PAIRING_PAYLOAD_VERSION, remotePairingPayload } from './pairing';

describe('pairing payload', () => {
  it('局域网载荷使用 snake_case 协议字段', () => {
    const payload = lanPairingPayload('http://192.168.1.5:27372', 'tok');
    expect(payload).toEqual({ v: PAIRING_PAYLOAD_VERSION, kind: 'lan', url: 'http://192.168.1.5:27372', token: 'tok' });
    expect(JSON.parse(encodePairingPayload(payload))).toEqual(payload);
  });

  it('广域网载荷携带 service_key 与 key', () => {
    const payload = remotePairingPayload('https://hawk.example.com', 'hs_service', 'hk_app');
    expect(payload).toEqual({
      v: PAIRING_PAYLOAD_VERSION,
      kind: 'remote',
      url: 'https://hawk.example.com',
      service_key: 'hs_service',
      key: 'hk_app',
    });
    expect(Object.keys(JSON.parse(encodePairingPayload(payload))).sort()).toEqual(['key', 'kind', 'service_key', 'url', 'v']);
  });

  it('版本号固定为 1（客户端据此判定协议兼容）', () => {
    expect(PAIRING_PAYLOAD_VERSION).toBe(1);
  });
});

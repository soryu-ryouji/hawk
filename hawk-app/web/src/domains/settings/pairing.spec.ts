import { describe, expect, it } from 'vitest';
import { encodePairingPayload, lanPairingPayload, PAIRING_PAYLOAD_VERSION } from './pairing';

describe('移动端配对载荷', () => {
  it('只包含局域网候选地址、访问密钥与库标识', () => {
    const payload = lanPairingPayload(['http://192.168.1.5:27375'], 'hk_app', 'lib_target');
    expect(payload).toEqual({
      v: PAIRING_PAYLOAD_VERSION,
      target_library_id: 'lib_target',
      access_key: 'hk_app',
      lan_endpoints: ['http://192.168.1.5:27375'],
    });
    expect(JSON.parse(encodePairingPayload(payload))).toEqual(payload);
  });

  it('协议版本为 3', () => {
    expect(PAIRING_PAYLOAD_VERSION).toBe(3);
  });
});

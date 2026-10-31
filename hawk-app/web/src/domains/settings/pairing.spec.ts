import { describe, expect, it } from 'vitest';
import { encodePairingPayload, lanPairingPayload, PAIRING_PAYLOAD_VERSION, remotePairingPayload } from './pairing';

describe('pairing payload', () => {
  it('LAN/WAN 共用 AccessKey 与 target_identity，不携带 tenant key', () => {
    const payload = remotePairingPayload('https://hawk.example.com/', 'hk_app', 'ep_target', 'd_target', 'lib_target', ['http://192.168.1.5:27375']);
    expect(payload).toEqual({
      v: PAIRING_PAYLOAD_VERSION,
      target_library_id: 'lib_target',
      target_device_id: 'd_target',
      target_identity: 'ep_target',
      access_key: 'hk_app',
      lan_endpoints: ['http://192.168.1.5:27375'],
      wan: { server_url: 'https://hawk.example.com' },
    });
    expect(JSON.parse(encodePairingPayload(payload))).toEqual(payload);
    expect(encodePairingPayload(payload)).not.toContain('tenant_key');
  });

  it('LAN 载荷可以只携带局域网端点', () => {
    const payload = lanPairingPayload(['http://192.168.1.5:27375'], 'hk_app', 'ep_target', 'lib_target');
    expect(payload).toEqual({
      v: PAIRING_PAYLOAD_VERSION,
      target_library_id: 'lib_target',
      target_device_id: undefined,
      target_identity: 'ep_target',
      access_key: 'hk_app',
      lan_endpoints: ['http://192.168.1.5:27375'],
    });
  });

  it('版本号固定为 2', () => {
    expect(PAIRING_PAYLOAD_VERSION).toBe(2);
  });
});

// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

const env = vi.hoisted(() => ({
  lanGetSettings: vi.fn(),
  appLan: vi.fn(),
}));

vi.mock('@/shared/lib/platform', () => ({
  hasShell: true,
  shell: { lanGetSettings: env.lanGetSettings },
}));
vi.mock('@/shared/api/endpoints', () => ({ api: { appLan: env.appLan } }));

import { useLanSettingsStore } from './lanSettings';

const dto = {
  enabled: false,
  port: 27372,
  peer_port: 27375,
  peer_id: 'peer_local',
  target_identity: 'ep_local',
  target_library_id: 'lib_local',
  name: '本机',
  library_name: '本机库',
  keys: [
    {
      id: 'key_local',
      label: '配对设备',
      access_key: '',
      role: 'editor' as const,
      created_at_ms: 1,
      peer_id: 'peer_remote',
    },
  ],
  peer_keys: [],
  active: false,
  peer_active: true,
};

beforeEach(() => {
  setActivePinia(createPinia());
  env.lanGetSettings.mockReset();
  env.appLan.mockReset();
  env.lanGetSettings.mockResolvedValue(dto);
});

describe('useLanSettingsStore', () => {
  it('本机 LAN 配置只走 Loopback IPC，不调用当前浏览库的 app/lan', async () => {
    const store = useLanSettingsStore();

    await store.load();

    expect(env.lanGetSettings).toHaveBeenCalledTimes(1);
    expect(env.appLan).not.toHaveBeenCalled();
    expect(store.error).toBeNull();
    expect(store.loaded).toBe(true);
    expect(store.peerPort).toBe('27375');
    expect(store.peerKeys).toEqual(dto.keys);

    await store.load();
    expect(env.lanGetSettings).toHaveBeenCalledTimes(1);
  });
});

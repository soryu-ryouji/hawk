// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { reactive } from 'vue';
import type { LanPeer } from '@/shared/types';

const shellMock = vi.hoisted(() => ({
  lanPair: vi.fn((_input: { peer: LanPeer; writable: boolean; reverseWritable: boolean }) =>
    Promise.resolve({ api_base: 'http://192.168.1.2:27375', token: 't', label: '目标', remote: true, kind: 'lan', peer_id: 'peer' }),
  ),
  lanPeers: vi.fn(() => Promise.resolve({ peers: [] })),
}));

vi.mock('@/shared/lib/platform', () => ({ shell: shellMock }));
vi.mock('@/domains/remote', () => ({
  ipcMessage: (error: unknown) => (error instanceof Error ? error.message : String(error)),
}));

import { useLanStore } from './index';

describe('useLanStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
  });

  it('pair 向 IPC 传纯对象，避免 Vue Proxy 触发 DataCloneError', async () => {
    const peer = reactive<LanPeer>({
      id: 'peer',
      name: '目标',
      library_name: '库',
      url: 'http://192.168.1.2:27375',
      writable: true,
      requires_pairing: true,
    });

    const store = useLanStore();
    await store.pair(peer, true, false);

    const payload = shellMock.lanPair.mock.calls[0][0];
    expect(() => structuredClone(payload)).not.toThrow();
    expect(payload.peer).not.toBe(peer);
    expect(payload.peer).toEqual({
      id: 'peer',
      name: '目标',
      library_name: '库',
      url: 'http://192.168.1.2:27375',
      writable: true,
      requires_pairing: true,
    });
    expect(payload.writable).toBe(true);
    expect(payload.reverseWritable).toBe(false);
  });
});

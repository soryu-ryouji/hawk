// @vitest-environment jsdom
// 远程域测试：store 动作（配置保存/会话连接/错误剥壳）与连接 label 单例。
// shell 经 vi.mock 注入——分层测试约定同 boot.spec.ts。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import type { ConnectionDescriptor, RemoteDto } from '@/shared/types';

const env = vi.hoisted(() => ({
  status: { enabled: true, url: 'https://hub', service_key: 'hs_k', writable: false, status: { state: 'online', device_id: 'd_self' } } as RemoteDto,
  putOk: true,
  connectError: null as string | null,
  connection: null as ConnectionDescriptor | null,
  connCbs: [] as Array<(conn: ConnectionDescriptor) => void>,
  keys: [] as Array<{ id: string; label: string; read: boolean; write: boolean; prefix: string; created_at_ms: number }>,
  createKeyOk: true,
}));

const shellMock = vi.hoisted(() => ({
  remoteStatus: vi.fn(() => Promise.resolve(env.status)),
  remotePutConfig: vi.fn(() => (env.putOk ? Promise.resolve(env.status) : Promise.reject(new Error('校验失败')))),
  remoteDevices: vi.fn(() =>
    Promise.resolve({
      devices: [
        { device_id: 'd_self', name: '本机', platform: 'macos', library_name: '库', online: true, allow_remote: true, allow_write: false },
        { device_id: 'd_off', name: '离线机', platform: 'windows', library_name: '库2', online: false, allow_remote: true, allow_write: false },
        { device_id: 'd_on', name: '台式机', platform: 'windows', library_name: '库3', online: true, allow_remote: true, allow_write: false },
      ],
    }),
  ),
  remoteConnect: vi.fn((_id: string, mode: string) =>
    env.connectError && mode === 'direct'
      ? Promise.reject(new Error(env.connectError))
      : Promise.resolve({ api_base: 'http://127.0.0.1:1', token: 's', label: _id, remote: true }),
  ),
  remoteDisconnect: vi.fn(() => Promise.resolve(null)),
  remotePatchDevice: vi.fn(() => Promise.resolve({})),
  remoteRotate: vi.fn(() => Promise.resolve({ fingerprint: 'fp' })),
  remoteKeys: vi.fn(() => Promise.resolve({ keys: env.keys })),
  remoteCreateKey: vi.fn(() =>
    env.createKeyOk
      ? Promise.resolve({ id: 'k_new', key: 'hk_plain-once', synced: true })
      : Promise.reject(new Error('自定义密钥至少 12 个字符（携带写权限）')),
  ),
  remotePatchKey: vi.fn((_id: string, patch: { read?: boolean; write?: boolean }) => {
    env.keys = env.keys.map((k) => (k.id === _id ? { ...k, ...patch } : k));
    return Promise.resolve({ key: env.keys.find((k) => k.id === _id)!, synced: true });
  }),
  remoteDeleteKey: vi.fn((id: string) => {
    env.keys = env.keys.filter((k) => k.id !== id);
    return Promise.resolve({ synced: true });
  }),
  remoteGetConnection: vi.fn(() => Promise.resolve(env.connection)),
  onConnectionChanged: vi.fn((cb: (conn: ConnectionDescriptor) => void) => {
    env.connCbs.push(cb);
    return () => {};
  }),
}));

vi.mock('@/shared/lib/platform', () => ({
  hasShell: true,
  isMac: false,
  platform: 'darwin',
  shell: shellMock,
}));

import { useRemoteStore, ipcMessage, initConnectionLabel, useConnectionLabel } from './index';

describe('ipcMessage', () => {
  it('剥掉 IPC invoke 的错误前缀', () => {
    expect(ipcMessage(new Error("Error invoking remote method 'hawk:remote-connect': Error: 对方拒绝"))).toBe('对方拒绝');
    expect(ipcMessage(new Error('普通错误'))).toBe('普通错误');
  });
});

describe('useRemoteStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.clearAllMocks();
    env.putOk = true;
    env.connectError = null;
  });

  it('refresh 填充配置与状态', async () => {
    const store = useRemoteStore();
    await store.refresh();
    expect(store.dto?.status.state).toBe('online');
    expect(store.error).toBeNull();
  });

  it('saveConfig 成功回写 dto；失败写入 error 并返回 false', async () => {
    const store = useRemoteStore();
    expect(await store.saveConfig({ enabled: true, url: 'https://hub', service_key: 'hs_k', writable: false })).toBe(true);
    expect(store.dto?.url).toBe('https://hub');

    env.putOk = false;
    expect(await store.saveConfig({ enabled: true, url: '', service_key: '', writable: false })).toBe(false);
    expect(store.error).toBe('校验失败');
  });

  it('connect 失败时 error 为剥壳后的服务端 message', async () => {
    const store = useRemoteStore();
    env.connectError = "Error invoking remote method 'hawk:remote-connect': Error: 目标网络不可达：无直连候选";
    await store.connect('d_x');
    expect(store.error).toBe('目标网络不可达：无直连候选（可在远程设置中开启「中继回退」）');
    expect(store.connectingId).toBeNull();
  });

  it('中继回退：开启后直连失败经确认走 relay 重试', async () => {
    const store = useRemoteStore();
    env.connectError = "Error invoking remote method 'hawk:remote-connect': Error: 直连失败：超时";
    vi.stubGlobal(
      'confirm',
      vi.fn(() => true),
    );

    // 默认关闭：不确认、不重试，error 带开启提示
    await store.connect('d_x');
    expect(shellMock.remoteConnect).toHaveBeenCalledTimes(1);
    expect(shellMock.remoteConnect).toHaveBeenLastCalledWith('d_x', 'direct');
    expect(store.error).toContain('中继回退');

    // 开启后：确认弹窗 → relay 重试成功，无 error
    store.setRelayFallback(true);
    await store.connect('d_x');
    expect(shellMock.remoteConnect).toHaveBeenLastCalledWith('d_x', 'relay');
    expect(store.error).toBeNull();
    expect(localStorage.getItem('hawk:relayFallback')).toBe('1');
    vi.unstubAllGlobals();
  });

  it('refreshDevices 离线排序与状态过滤（信令未 online 时不拉取）', async () => {
    const store = useRemoteStore();
    store.dto = { ...env.status, status: { state: 'connecting' } };
    await store.refreshDevices();
    expect(store.devices).toEqual([]);

    store.dto = env.status;
    await store.refreshDevices();
    expect(store.devices.map((d) => d.device_id)).toContain('d_on');
  });

  it('访问密钥：签发返回明文并刷新列表；失败写 error', async () => {
    const store = useRemoteStore();
    const created = await store.createKey({ label: '插件', read: true });
    expect(created?.key).toBe('hk_plain-once');
    expect(shellMock.remoteCreateKey).toHaveBeenCalledWith({ label: '插件', read: true });

    env.createKeyOk = false;
    const failed = await store.createKey({ read: true, write: true, custom: 'short' });
    expect(failed).toBeNull();
    expect(store.error).toContain('至少 12 个字符');
    env.createKeyOk = true;
  });

  it('访问密钥：改权限/吊销后列表同步', async () => {
    const store = useRemoteStore();
    store.dto = env.status;
    env.keys = [{ id: 'k_1', label: '插件', read: true, write: false, prefix: 'hk_a', created_at_ms: 1 }];
    await store.refreshKeys();
    expect(store.keys).toHaveLength(1);

    await store.patchKey('k_1', { write: true });
    expect(store.keys[0].write).toBe(true);

    await store.deleteKey('k_1');
    expect(store.keys).toHaveLength(0);
  });

  it('refreshKeys 远程未启用时清空', async () => {
    const store = useRemoteStore();
    store.dto = { ...env.status, enabled: false };
    env.keys = [{ id: 'k_1', label: '', read: true, write: false, prefix: 'hk_', created_at_ms: 1 }];
    await store.refreshKeys();
    expect(store.keys).toEqual([]);
  });
});

describe('useConnectionLabel', () => {
  it('init 应用当前连接并响应变更', async () => {
    setActivePinia(createPinia());
    env.connection = { api_base: 'http://127.0.0.1:1', token: 't', label: '我的库', remote: false };
    initConnectionLabel();
    await Promise.resolve();
    const { label, isRemote } = useConnectionLabel();
    expect(label.value).toBe('我的库');
    expect(isRemote.value).toBe(false);

    // 主进程推送切换到远程
    const remote: ConnectionDescriptor = { api_base: 'http://127.0.0.1:2', token: 's', label: '台式机', remote: true };
    for (const cb of env.connCbs) {
      cb(remote);
    }
    expect(label.value).toBe('台式机');
    expect(isRemote.value).toBe(true);
  });
});

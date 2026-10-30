// @vitest-environment jsdom
// 远程域测试：store 动作（配置保存/会话连接/错误剥壳）与连接 label 单例。
// shell 经 vi.mock 注入——分层测试约定同 boot.spec.ts。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import type { ConnectionDescriptor, RemoteDto } from '@/shared/types';

const env = vi.hoisted(() => ({
  status: {
    enabled: true,
    url: 'https://hub',
    service_key: 'hs_k',
    allow_import: false,
    relay: false,
    status: { state: 'online', device_id: 'd_self' },
  } as RemoteDto,
  putOk: true,
  connectError: null as string | null,
  connection: null as ConnectionDescriptor | null,
  connCbs: [] as Array<(conn: ConnectionDescriptor) => void>,
  keys: [] as Array<{ id: string; label: string; role: string; prefix: string; created_at_ms: number }>,
  createKeyOk: true,
}));

const shellMock = vi.hoisted(() => ({
  remoteStatus: vi.fn(() => Promise.resolve(env.status)),
  remotePutConfig: vi.fn((config: Record<string, unknown>) =>
    env.putOk ? Promise.resolve({ ...env.status, ...config } as RemoteDto) : Promise.reject(new Error('校验失败')),
  ),
  remoteDevices: vi.fn(() =>
    Promise.resolve({
      devices: [
        { device_id: 'd_self', name: '本机', platform: 'macos', library_name: '库', online: true, allow_remote: true, allow_import: false },
        { device_id: 'd_off', name: '离线机', platform: 'windows', library_name: '库2', online: false, allow_remote: true, allow_import: false },
        { device_id: 'd_on', name: '台式机', platform: 'windows', library_name: '库3', online: true, allow_remote: true, allow_import: false },
      ],
    }),
  ),
  remoteConnect: vi.fn((_id: string) =>
    env.connectError
      ? Promise.reject(new Error(env.connectError))
      : Promise.resolve({ api_base: 'http://127.0.0.1:1', credential: 's', label: _id, remote: true, kind: 'remote' }),
  ),
  remoteDisconnect: vi.fn(() => Promise.resolve(null)),
  remotePatchDevice: vi.fn(() => Promise.resolve({})),
  remoteRotate: vi.fn(() => Promise.resolve({ endpoint_id: 'ep' })),
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
    expect(await store.saveConfig({ enabled: true, url: 'https://hub', service_key: 'hs_k', allow_import: false, relay: false })).toBe(true);
    expect(store.dto?.url).toBe('https://hub');

    env.putOk = false;
    expect(await store.saveConfig({ enabled: true, url: '', service_key: '', allow_import: false, relay: false })).toBe(false);
    expect(store.error).toBe('校验失败');
  });

  it('connect 失败时 error 为剥壳后的服务端 message', async () => {
    const store = useRemoteStore();
    env.connectError = "Error invoking remote method 'hawk:remote-connect': Error: 目标网络不可达：无直连候选";
    await store.connect('d_x');
    expect(store.error).toBe('目标网络不可达：无直连候选（可在远程设置中开启「使用中继」）');
    expect(store.connectingId).toBeNull();
  });

  it('连接失败：错误剥壳；中继开关关闭时附开启引导、开启后原样展示', async () => {
    const store = useRemoteStore();
    store.dto = { ...env.status }; // relay: false
    env.connectError = "Error invoking remote method 'hawk:remote-connect': Error: 直连失败：超时";

    // 默认关闭：单次连接（无 mode 参数），error 带开启引导
    await store.connect('d_x');
    expect(shellMock.remoteConnect).toHaveBeenCalledTimes(1);
    expect(shellMock.remoteConnect).toHaveBeenLastCalledWith('d_x');
    expect(store.error).toBe('直连失败：超时（可在远程设置中开启「使用中继」）');

    // 开启后（开关写回 daemon 配置）：错误消息原样（无引导后缀）
    await store.setRelayEnabled(true);
    expect(shellMock.remotePutConfig).toHaveBeenLastCalledWith(expect.objectContaining({ relay: true }));
    await store.connect('d_x');
    expect(store.error).toBe('直连失败：超时');
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
    const created = await store.createKey({ label: '插件', role: 'viewer' });
    expect(created?.key).toBe('hk_plain-once');
    expect(shellMock.remoteCreateKey).toHaveBeenCalledWith({ label: '插件', role: 'viewer' });

    env.createKeyOk = false;
    const failed = await store.createKey({ role: 'importer', custom: 'short' });
    expect(failed).toBeNull();
    expect(store.error).toContain('至少 12 个字符');
    env.createKeyOk = true;
  });

  it('访问密钥：改权限/吊销后列表同步', async () => {
    const store = useRemoteStore();
    store.dto = env.status;
    env.keys = [{ id: 'k_1', label: '插件', role: 'viewer', prefix: 'hk_a', created_at_ms: 1 }];
    await store.refreshKeys();
    expect(store.keys).toHaveLength(1);

    await store.patchKey('k_1', { role: 'importer' });
    expect(store.keys[0].role).toBe('importer');

    await store.deleteKey('k_1');
    expect(store.keys).toHaveLength(0);
  });

  it('refreshKeys 不依赖 dto 就绪/启用状态（密钥本地持久保管；挂载竞态回归）', async () => {
    const store = useRemoteStore();
    env.keys = [{ id: 'k_1', label: '', role: 'viewer', prefix: 'hk_', created_at_ms: 1 }];
    // dto 未拉取（null）与远程未启用两种形态下都必须照常列出
    store.dto = null;
    await store.refreshKeys();
    expect(store.keys).toHaveLength(1);

    store.dto = { ...env.status, enabled: false };
    await store.refreshKeys();
    expect(store.keys).toHaveLength(1);
    env.keys = [];
  });
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
  const created = await store.createKey({ label: '插件', role: 'viewer' });
  expect(created?.key).toBe('hk_plain-once');
  expect(shellMock.remoteCreateKey).toHaveBeenCalledWith({ label: '插件', role: 'viewer' });

  env.createKeyOk = false;
  const failed = await store.createKey({ role: 'importer', custom: 'short' });
  expect(failed).toBeNull();
  expect(store.error).toContain('至少 12 个字符');
  env.createKeyOk = true;
});

it('访问密钥：改权限/吊销后列表同步', async () => {
  const store = useRemoteStore();
  store.dto = env.status;
  env.keys = [{ id: 'k_1', label: '插件', role: 'viewer', prefix: 'hk_a', created_at_ms: 1 }];
  await store.refreshKeys();
  expect(store.keys).toHaveLength(1);

  await store.patchKey('k_1', { role: 'importer' });
  expect(store.keys[0].role).toBe('importer');

  await store.deleteKey('k_1');
  expect(store.keys).toHaveLength(0);
});

it('refreshKeys 不依赖 dto 就绪/启用状态（密钥本地持久保管；挂载竞态回归）', async () => {
  const store = useRemoteStore();
  env.keys = [{ id: 'k_1', label: '', role: 'viewer', prefix: 'hk_', created_at_ms: 1 }];
  // dto 未拉取（null）与远程未启用两种形态下都必须照常列出
  store.dto = null;
  await store.refreshKeys();
  expect(store.keys).toHaveLength(1);

  store.dto = { ...env.status, enabled: false };
  await store.refreshKeys();
  expect(store.keys).toHaveLength(1);
  env.keys = [];
});
describe('useConnectionLabel', () => {
  it('init 应用当前连接并响应变更', async () => {
    setActivePinia(createPinia());
    env.connection = { api_base: 'http://127.0.0.1:1', credential: 't', label: '我的库', remote: false, kind: 'local' };
    initConnectionLabel();
    await Promise.resolve();
    const { label, isRemote } = useConnectionLabel();
    expect(label.value).toBe('我的库');
    expect(isRemote.value).toBe(false);

    // 主进程推送切换到远程
    const remote: ConnectionDescriptor = { api_base: 'http://127.0.0.1:2', credential: 's', label: '台式机', remote: true, kind: 'remote' };
    for (const cb of env.connCbs) {
      cb(remote);
    }
    expect(label.value).toBe('台式机');
    expect(isRemote.value).toBe(true);
  });
});

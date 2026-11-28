// @vitest-environment jsdom
// 启动相位机测试（refactor-plan.md 阶段 2）：相位迁移与 boot 编排接线。
// platform（shell 适配器）与 api 经 mock 注入——分层测试约定的 app 层测试形态。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { defineComponent } from 'vue';
import { mount } from '@vue/test-utils';

const env = vi.hoisted(() => ({
  hasShell: true,
  initOk: true,
  restartCb: null as null | (() => void),
  errorCb: null as null | ((e: { message: string; code?: string; port?: number }) => void),
  lostCb: null as null | ((n: { label: string; peer_id?: string }) => void),
  restoredCb: null as null | ((n: { label: string; peer_id?: string }) => void),
  connectionState: { reconnecting: false, label: null as string | null },
  pullError: null as null | { message: string; code?: string; port?: number },
  clearStoredCredential: vi.fn(),
  lanSettingsLoad: vi.fn(() => Promise.resolve()),
  connectEvents: vi.fn((..._args: unknown[]) => vi.fn()),
}));

vi.mock('@/shared/api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/shared/api/client')>();
  return {
    ...actual,
    initApi: () => (env.initOk ? { api: 'http://127.0.0.1:1', credential: 't' } : null),
    apiConfig: () => ({ api: 'http://127.0.0.1:1', credential: 't' }),
    clearStoredCredential: env.clearStoredCredential,
  };
});

const apiMocks = vi.hoisted(() => ({
  startupStatus: vi.fn(() => Promise.resolve({ status: 'ready' as const })),
  appInfo: vi.fn(() => Promise.resolve({ role: 'admin', channel: 'loopback', has_library: true })),
  appSession: vi.fn(() => Promise.resolve({ role: 'viewer', channel: 'lan_web', expires_in: 100 })),
  libraryInfo: vi.fn(() => Promise.resolve({ name: '库', path: '/lib' })),
  viewPreferences: vi.fn(() => Promise.resolve({})),
  itemSkeleton: vi.fn(() => Promise.resolve({ items: [], total_size: 0 })),
  folderList: vi.fn(() => Promise.resolve([])),
  categoryList: vi.fn(() => Promise.resolve([])),
  tagList: vi.fn(() => Promise.resolve([])),
  globalFilterList: vi.fn(() => Promise.resolve({ folders: [], categories: [], tags: [] })),
}));
vi.mock('@/shared/api/endpoints', () => ({ api: apiMocks }));

vi.mock('@/shared/api/events', () => ({ connectEvents: env.connectEvents }));
vi.mock('@/domains/settings', () => ({ useLanSettingsStore: () => ({ load: env.lanSettingsLoad }) }));

vi.mock('@/shared/lib/platform', () => ({
  get hasShell() {
    return env.hasShell;
  },
  isMac: false,
  platform: 'win32',
  shell: {
    onServerRestarting: (cb: () => void) => {
      env.restartCb = cb;
      return () => {};
    },
    onServerProgress: () => () => {},
    onServerError: (cb: (e: { message: string; code?: string; port?: number }) => void) => {
      env.errorCb = cb;
      return () => {};
    },
    onServerStarted: () => () => {},
    getServerConn: () => Promise.resolve(null),
    getServerError: () => Promise.resolve(env.pullError),
    getConnection: () => Promise.resolve(null),
    getConnectionState: () => Promise.resolve(env.connectionState),
    onConnectionChanged: () => () => {},
    onConnectionLost: (cb: (n: { label: string; peer_id?: string }) => void) => {
      env.lostCb = cb;
      return () => {};
    },
    onConnectionRestored: (cb: (n: { label: string; peer_id?: string }) => void) => {
      env.restoredCb = cb;
      return () => {};
    },
    quitApp: () => {},
  },
}));

import { useBoot } from './boot';
import { ApiError } from '@/shared/api/client';

/** 挂载宿主组件取得 useBoot 实例（相位机依赖组件生命周期注册 shell 监听） */
function mountBoot(): ReturnType<typeof useBoot> {
  let boot!: ReturnType<typeof useBoot>;
  const Host = defineComponent({
    setup() {
      boot = useBoot();
      return () => null;
    },
  });
  mount(Host);
  return boot;
}

/** 逐微任务冲刷（boot 链路全为 microtask，无 timer 依赖） */
async function flush(ticks = 20) {
  for (let i = 0; i < ticks; i++) {
    await Promise.resolve();
  }
}

beforeEach(() => {
  setActivePinia(createPinia());
  env.hasShell = true;
  env.initOk = true;
  env.restartCb = null;
  env.errorCb = null;
  env.lostCb = null;
  env.restoredCb = null;
  env.connectionState = { reconnecting: false, label: null };
  env.pullError = null;
  env.clearStoredCredential.mockClear();
  env.lanSettingsLoad.mockClear();
  env.connectEvents.mockClear();
  for (const fn of Object.values(apiMocks)) fn.mockClear();
  apiMocks.startupStatus.mockImplementation(() => Promise.resolve({ status: 'ready' as const }));
});

describe('启动相位机', () => {
  it('Electron 无连接参数进 setup，浏览器进 error（初始判定先于轮询监听）', () => {
    env.initOk = false;
    env.hasShell = true;
    const boot = mountBoot();
    expect(boot.phase.value).toBe('setup');
    expect(boot.bootError.value).toBeNull();

    env.initOk = false;
    env.hasShell = false;
    const boot2 = mountBoot();
    expect(boot2.phase.value).toBe('error');
    expect(boot2.bootError.value).toBe('缺少后端连接参数');
    expect(apiMocks.startupStatus).not.toHaveBeenCalled(); // 不对 error 相位发起注定失败的轮询
  });

  it('runBoot 数据拉取 UNAUTHORIZED：清 token 转门页 connect', async () => {
    apiMocks.appInfo.mockImplementationOnce(() => Promise.reject(new ApiError('UNAUTHORIZED', '未授权', 401)));
    const boot = mountBoot();
    expect(boot.phase.value).toBe('starting');

    await boot.runBoot();

    expect(boot.phase.value).toBe('connect');
    expect(env.clearStoredCredential).toHaveBeenCalledWith('http://127.0.0.1:1');
  });

  it('端口被占用（推送）：错误屏带 code/port，供启动屏显示换端口界面', async () => {
    const boot = mountBoot();
    env.errorCb?.({ message: '本地端口 27371 已被占用，本地服务无法启动', code: 'PORT_IN_USE', port: 27371 });
    await flush();
    expect(boot.phase.value).toBe('error');
    expect(boot.bootFailure.value?.code).toBe('PORT_IN_USE');
    expect(boot.bootFailure.value?.port).toBe(27371);
  });

  it('端口被占用（页面订阅晚于失败）：挂载时拉取兜底同样进错误屏', async () => {
    env.pullError = { message: '本地端口 27371 已被占用，本地服务无法启动', code: 'PORT_IN_USE', port: 27371 };
    const boot = mountBoot();
    await flush();
    expect(boot.phase.value).toBe('error');
    expect(boot.bootFailure.value?.code).toBe('PORT_IN_USE');
  });

  it('runBoot 一般失败：进错误屏并保留原因', async () => {
    apiMocks.appInfo.mockImplementationOnce(() => Promise.reject(new Error('daemon 崩了')));
    const boot = mountBoot();
    await boot.runBoot();
    expect(boot.phase.value).toBe('error');
    expect(boot.bootError.value).toBe('daemon 崩了');
  });

  it('server 重启事件：就绪相位立即回落启动屏（换库窗口期不假死）', () => {
    const boot = mountBoot();
    boot.phase.value = 'ready';
    env.restartCb!();
    expect(boot.phase.value).toBe('starting');
  });

  it('局域网断线：进入恢复等待；恢复后重新 boot 回主界面', async () => {
    const boot = mountBoot();
    await boot.runBoot();
    expect(boot.phase.value).toBe('ready');

    env.lostCb?.({ label: '客厅的库' });
    expect(boot.phase.value).toBe('reconnecting');
    expect(boot.recoveryLabel.value).toBe('客厅的库');

    env.restoredCb?.({ label: '客厅的库' });
    await flush();
    expect(boot.phase.value).toBe('ready');
    expect(env.connectEvents).toHaveBeenCalledTimes(2);
  });

  it('页面加载时已在恢复等待：直接进入恢复界面', async () => {
    env.connectionState = { reconnecting: true, label: '书房' };
    const boot = mountBoot();
    await flush();
    expect(boot.phase.value).toBe('reconnecting');
    expect(boot.recoveryLabel.value).toBe('书房');
  });

  it('浏览器路径：就绪即轮询 → boot 编排 → 连接 SSE → ready', async () => {
    env.hasShell = false;
    const boot = mountBoot();
    await flush();

    expect(apiMocks.startupStatus).toHaveBeenCalled();
    expect(apiMocks.folderList).toHaveBeenCalled(); // taxonomy 先行
    expect(apiMocks.appInfo).toHaveBeenCalledTimes(1); // runBoot 预检结果直接交给 store.init，不重复请求
    expect(env.connectEvents).toHaveBeenCalledTimes(1);
    expect(boot.phase.value).toBe('ready');
  });

  it('lan.access.changed：刷新 LAN 设置页的 AccessKey 缓存', async () => {
    const boot = mountBoot();
    await boot.runBoot();
    expect(boot.phase.value).toBe('ready');
    const handlers = env.connectEvents.mock.calls[0][0] as import('@/shared/api/events').EventHandlers;

    handlers.onLanAccessChanged();
    await flush();

    expect(env.lanSettingsLoad).toHaveBeenCalledWith(true);
  });
});

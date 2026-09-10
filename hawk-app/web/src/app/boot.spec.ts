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
  clearStoredToken: vi.fn(),
  connectEvents: vi.fn(() => vi.fn()),
}));

vi.mock('@/shared/api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/shared/api/client')>();
  return {
    ...actual,
    initApi: () => (env.initOk ? { api: 'http://127.0.0.1:1', token: 't' } : null),
    apiConfig: () => ({ api: 'http://127.0.0.1:1', token: 't' }),
    clearStoredToken: env.clearStoredToken,
  };
});

const apiMocks = vi.hoisted(() => ({
  startupStatus: vi.fn(() => Promise.resolve({ status: 'ready' as const })),
  appInfo: vi.fn(() => Promise.resolve({ access: 'admin', writable: true })),
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
    onServerError: () => () => {},
    onServerStarted: () => () => {},
    getServerConn: () => Promise.resolve(null),
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
  env.clearStoredToken.mockClear();
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
    expect(env.clearStoredToken).toHaveBeenCalledWith('http://127.0.0.1:1');
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

  it('浏览器路径：就绪即轮询 → boot 编排 → 连接 SSE → ready', async () => {
    env.hasShell = false;
    const boot = mountBoot();
    await flush();

    expect(apiMocks.startupStatus).toHaveBeenCalled();
    expect(apiMocks.folderList).toHaveBeenCalled(); // taxonomy 先行
    expect(apiMocks.appInfo).toHaveBeenCalled(); // store.init
    expect(env.connectEvents).toHaveBeenCalledTimes(1);
    expect(boot.phase.value).toBe('ready');
  });
});

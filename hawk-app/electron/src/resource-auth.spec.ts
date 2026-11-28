// 资源请求鉴权：<img>/EventSource 无法设置 Authorization，主进程按当前 API 同源注入。
import { describe, expect, it, vi } from 'vitest';

const env = vi.hoisted(() => ({
  filter: null as null | { urls: string[] },
  handler: null as null | ((details: { url: string; requestHeaders: Record<string, string> }, callback: (arg: unknown) => void) => void),
}));

vi.mock('electron', () => ({
  session: {
    defaultSession: {
      webRequest: {
        onBeforeSendHeaders: (filter: { urls: string[] }, handler: typeof env.handler) => {
          env.filter = filter;
          env.handler = handler;
        },
      },
    },
  },
}));

import { setResourceAuth } from './resource-auth';

describe('setResourceAuth', () => {
  it('监听所有 http(s)，但只对当前 API 同源请求注入 Bearer', () => {
    setResourceAuth('http://192.168.1.5:27375', 'hk_test');
    expect(env.filter?.urls).toEqual(['http://*/*', 'https://*/*']);

    const callback = vi.fn();
    env.handler?.({ url: 'http://192.168.1.5:27375/api/v1/item/thumbnail?id=abc', requestHeaders: {} }, callback);
    expect(callback).toHaveBeenCalledWith({ requestHeaders: { Authorization: 'Bearer hk_test' } });

    const other = vi.fn();
    env.handler?.({ url: 'http://192.168.1.6:27375/api/v1/item/thumbnail?id=abc', requestHeaders: {} }, other);
    expect(other).toHaveBeenCalledWith({ requestHeaders: {} });
  });
});

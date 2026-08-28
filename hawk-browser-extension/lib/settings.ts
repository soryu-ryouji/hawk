// 插件设置（browser.storage.local 持久化）：服务端地址 + Token
import { browser } from 'wxt/browser';

export interface HawkSettings {
  /** hawk-server 地址，默认本机 27371 端口（见 docs/architecture.md） */
  serverUrl: string;
  /** 手动覆盖 Token（可选）；留空则自动发现（见 lib/api.ts 头部说明） */
  token: string;
}

const DEFAULTS: HawkSettings = {
  serverUrl: 'http://127.0.0.1:27371',
  token: '',
};

export async function getSettings(): Promise<HawkSettings> {
  const stored = await browser.storage.local.get('settings');
  return { ...DEFAULTS, ...((stored.settings ?? {}) as Partial<HawkSettings>) };
}

export async function saveSettings(patch: Partial<HawkSettings>): Promise<HawkSettings> {
  const normalized = patch.serverUrl !== undefined ? { ...patch, serverUrl: patch.serverUrl.trim().replace(/\/+$/, '') } : patch;
  const next = { ...(await getSettings()), ...normalized };
  await browser.storage.local.set({ settings: next });
  return next;
}

// 插件设置（browser.storage.local 持久化）：只连接本机 hawk daemon。
import { browser } from 'wxt/browser';

export interface HawkSettings {
  /** 本地 daemon 端口覆盖（高级选项，留空 = 自动发现 27371–27400）；正常情况下不需要填 */
  localPort: string;
  /** 手动覆盖本地 Token（高级选项，可选）；留空则自动发现（仅环回地址） */
  localToken: string;
}

const DEFAULTS: HawkSettings = {
  localPort: '',
  localToken: '',
};

export async function getSettings(): Promise<HawkSettings> {
  const stored = await browser.storage.local.get('settings');
  return { ...DEFAULTS, ...((stored.settings ?? {}) as Partial<HawkSettings>) };
}

export async function saveSettings(patch: Partial<HawkSettings>): Promise<HawkSettings> {
  const normalized = { ...patch };
  if (patch.localPort !== undefined) {
    normalized.localPort = patch.localPort.trim();
  }
  if (patch.localToken !== undefined) {
    normalized.localToken = patch.localToken.trim();
  }
  const current = await getSettings();
  const next = { ...current, ...normalized };
  await browser.storage.local.set({ settings: next });
  return next;
}

// 插件设置（browser.storage.local 持久化）：本地 daemon / hawk-server 远程两种连接方式。
import { browser } from 'wxt/browser';

export type ConnectionMode = 'local' | 'remote';

export interface HawkSettings {
  mode: ConnectionMode;
  /** 本地 daemon 端口覆盖（高级选项，留空 = 自动发现 27371–27400）；正常情况下不需要填 */
  localPort: string;
  /** 手动覆盖本地 Token（高级选项，可选）；留空则自动发现（仅环回地址） */
  localToken: string;
  /** hawk-server 域名（http(s):// 开头） */
  remoteUrl: string;
  /** 服务密钥（service-key）：池门禁凭证，hawk-server tenant add 生成 */
  serviceKey: string;
  /** 访问密钥（hawk-app key，须带写权限）： hawk 设置 → 远程中签发，逐把吊销 */
  remoteKey: string;
  /** 写作用域令牌缓存（服务端重启/注销后 401 会自动重登） */
  remoteToken: string;
  /** 目标素材库 = hawk-server 上的一台在线设备 */
  targetDeviceId: string;
  targetName: string;
  targetLibraryName: string;
}

const DEFAULTS: HawkSettings = {
  mode: 'local',
  localPort: '',
  localToken: '',
  remoteUrl: '',
  serviceKey: '',
  remoteKey: '',
  remoteToken: '',
  targetDeviceId: '',
  targetName: '',
  targetLibraryName: '',
};

export async function getSettings(): Promise<HawkSettings> {
  const stored = await browser.storage.local.get('settings');
  return { ...DEFAULTS, ...((stored.settings ?? {}) as Partial<HawkSettings>) };
}

function normalizeUrl(value: string): string {
  return value.trim().replace(/\/+$/, '');
}

export async function saveSettings(patch: Partial<HawkSettings>): Promise<HawkSettings> {
  const normalized = { ...patch };
  if (patch.localPort !== undefined) {
    normalized.localPort = patch.localPort.trim();
  }
  if (patch.remoteUrl !== undefined) {
    normalized.remoteUrl = normalizeUrl(patch.remoteUrl);
  }
  if (patch.remoteKey !== undefined) {
    normalized.remoteKey = patch.remoteKey.trim();
  }
  if (patch.serviceKey !== undefined) {
    normalized.serviceKey = patch.serviceKey.trim();
  }
  if (patch.localToken !== undefined) {
    normalized.localToken = patch.localToken.trim();
  }
  const current = await getSettings();
  const next = { ...current, ...normalized };
  // 密钥/域名变化后旧令牌不再可信，清掉缓存（下次请求自动重登）
  const connectionChanged =
    (patch.remoteKey !== undefined && next.remoteKey !== current.remoteKey) ||
    (patch.serviceKey !== undefined && next.serviceKey !== current.serviceKey) ||
    (patch.remoteUrl !== undefined && next.remoteUrl !== current.remoteUrl);
  if (connectionChanged) {
    next.remoteToken = '';
  }
  await browser.storage.local.set({ settings: next });
  return next;
}

/** 当前目标素材库展示名（远程模式用；本地模式的实际目标由 daemon 的 app/connection 决定） */
export function targetLabel(settings: HawkSettings): string {
  if (!settings.targetDeviceId) {
    return '';
  }
  const name = settings.targetName || settings.targetDeviceId;
  return settings.targetLibraryName ? `${name} · ${settings.targetLibraryName}` : name;
}

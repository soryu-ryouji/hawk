// 连接编排：本地库 + 局域网直连（纯 HTTP）。
// 主进程始终持有本地 daemon 的地址与 admin token；切换局域网目标只是把前端 API base
// 指向目标 daemon，不再经过本机会话代理或加密隧道。
import os from 'node:os';
import { lanAddresses } from './lan';
import { getStartedConn } from './server';
import { getMainWindow } from './window';
import { setResourceAuth } from './resource-auth';
import { getLanTarget, getStoredLanPeer, listLanPeers, removeStoredLanPeer, setLanTarget, upsertLanPeer } from './app-config';
import {
  IPC,
  type AccessRole,
  type ConnectionDescriptor,
  type ConnectionNotice,
  type ConnectionState,
  type LanAccessKey,
  type LanClient,
  type LanPairRequest,
  type LanPeer,
  type LanPeerProfile,
  type LanSettings,
} from './ipc-contract';

let lanConn: ConnectionDescriptor | null = null;
let restoring = false;
/** 局域网断开后保持目标并每 10s 探测；只有用户显式切本地/忘记设备才清 target。 */
let lanReconnecting = false;
const LAN_MONITOR_INTERVAL_MS = 10_000;

function localDescriptor(): ConnectionDescriptor | null {
  const conn = getStartedConn();
  return conn ? { api_base: conn.address, credential: conn.admin_token, label: '我的库', remote: false, kind: 'local' } : null;
}

/** 当前活动连接描述符：局域网直连优先，其次本地库。 */
export function currentDescriptor(): ConnectionDescriptor | null {
  return lanConn ?? (restoring ? null : localDescriptor());
}

/** 局域网连接健康状态：页面晚于事件加载时用它恢复恢复等待界面。 */
export function connectionState(): ConnectionState {
  return { reconnecting: lanReconnecting, label: lanConn?.label ?? null };
}

function emitConnectionNotice(channel: string): void {
  if (!lanConn) {
    return;
  }
  const notice: ConnectionNotice = { label: lanConn.label, peer_id: lanConn.peer_id };
  getMainWindow()?.webContents.send(channel, notice);
}

function emitConnection(descriptor: ConnectionDescriptor): void {
  setResourceAuth(descriptor.api_base, descriptor.credential);
  getMainWindow()?.webContents.send(IPC.connectionChanged, descriptor);
}

function switchToLocal(): void {
  if (lanConn === null) {
    return;
  }
  lanConn = null;
  setLanTarget(null);
  stopLanMonitor();
  const local = localDescriptor();
  if (local) {
    emitConnection(local);
  }
}

export function onServerReady(): void {
  // 本地 daemon 就绪不需要额外订阅；冷启动恢复由 server.ts 的 notifyReady 处理。
}

export function onServerStopped(): void {
  // 局域网直连不依赖本地 daemon；本地 daemon 重启/停止时保持当前目标，
  // 避免把正在浏览的局域网素材库静默切回本地。
}

// ---------- daemon REST（本地环回，admin token） ----------

async function daemonCall<T>(method: string, path: string, body?: unknown): Promise<T> {
  const conn = getStartedConn();
  if (!conn) {
    throw new Error('本地服务未就绪');
  }
  const res = await fetch(`${conn.address}${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${conn.admin_token}`,
      'Content-Type': 'application/json',
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const envelope = (await res.json().catch(() => null)) as {
    status: string;
    data?: T;
    error?: { message?: string };
  } | null;
  if (!res.ok || !envelope || envelope.status === 'error') {
    throw new Error(envelope?.error?.message ?? `HTTP ${res.status}`);
  }
  return envelope.data as T;
}

async function directCall<T>(base: string, path: string, token: string, method = 'GET', body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${base}${path}`, {
      method,
      headers: {
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
      signal: AbortSignal.timeout(6000),
    });
  } catch {
    throw new Error('无法连接局域网设备（确认在同一网络且目标已启用局域网访问）');
  }
  const envelope = (await res.json().catch(() => null)) as {
    status: string;
    data?: T;
    error?: { message?: string };
  } | null;
  if (!res.ok || !envelope || envelope.status === 'error') {
    throw new Error(envelope?.error?.message ?? `HTTP ${res.status}`);
  }
  return envelope.data as T;
}

interface TargetAppInfo {
  has_library: boolean;
  role: AccessRole;
  channel: 'loopback' | 'lan_web' | 'lan_peer';
}

function normalizeLanURL(url: string): string {
  const candidate = /^https?:\/\//i.test(url.trim()) ? url.trim() : `http://${url.trim()}`;
  const parsed = new URL(candidate);
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new Error('局域网设备地址必须使用 http/https');
  }
  return parsed.toString().replace(/\/+$/, '');
}

// ---------- 局域网管理面 ----------

export async function lanGetSettings(): Promise<LanSettings> {
  return daemonCall<LanSettings>('GET', '/api/v1/app/lan');
}

export async function lanDiscover(): Promise<{ peers: LanPeer[] }> {
  return daemonCall<{ peers: LanPeer[] }>('GET', '/api/v1/app/lan/peers');
}

export async function lanPeers(): Promise<{ peers: LanPeerProfile[] }> {
  return { peers: listLanPeers() };
}

export async function lanSpeedTest(): Promise<{ latency_ms: number }> {
  const peerId = lanConn?.peer_id;
  if (!peerId) {
    throw new Error('当前未连接局域网设备');
  }
  const profile = getStoredLanPeer(peerId);
  if (!profile) {
    throw new Error('局域网设备记录不存在，请重新配对');
  }
  await directCall<TargetAppInfo>(profile.url, '/api/v1/app/info', profile.peer_key);
  const samples: number[] = [];
  for (let i = 0; i < 5; i += 1) {
    const started = Date.now();
    await directCall<TargetAppInfo>(profile.url, '/api/v1/app/info', profile.peer_key);
    samples.push(Date.now() - started);
  }
  samples.sort((a, b) => a - b);
  return { latency_ms: samples[Math.floor(samples.length / 2)] };
}

interface ReversePeerProfile {
  peer_id: string;
  name: string;
  library_name: string;
  urls: string[];
  peer_key: string;
  role: AccessRole;
  target_library_id: string;
}

interface LanPairedPeer {
  id: string;
  name: string;
  library_name: string;
  url: string;
  role: AccessRole;
  peer_key: string;
  target_library_id: string;
}

async function addReversePeerKey(target: LanPeer, writable: boolean): Promise<{ keyId: string; profile: ReversePeerProfile }> {
  const key = await daemonCall<LanAccessKey>('POST', '/api/v1/app/lan/device-keys', {
    label: target.name,
    role: writable ? 'editor' : 'viewer',
    peer_id: target.id,
    library_name: target.library_name,
    url: target.url,
  });
  try {
    const settings = await daemonCall<LanSettings>('GET', '/api/v1/app/lan');
    return {
      keyId: key.id,
      profile: {
        peer_id: settings.peer_id,
        name: settings.name,
        library_name: settings.library_name,
        urls: lanAddresses().map((ip) => 'http://' + ip + ':' + settings.peer_port),
        peer_key: key.access_key,
        role: key.role,
        target_library_id: settings.target_library_id,
      },
    };
  } catch (error) {
    await removeReversePeerKey(key.id).catch(() => {});
    throw error;
  }
}

async function removeReversePeerKey(keyId: string): Promise<void> {
  await daemonCall('DELETE', '/api/v1/app/lan/device-keys/' + encodeURIComponent(keyId));
}

export async function lanPairRequests(): Promise<{ requests: LanPairRequest[] }> {
  return daemonCall<{ requests: LanPairRequest[] }>('GET', '/api/v1/app/lan/pair-requests');
}

export async function lanClients(): Promise<{ clients: LanClient[] }> {
  return daemonCall<{ clients: LanClient[] }>('GET', '/api/v1/app/lan/clients');
}

export async function lanDecidePairRequest(requestId: string, body: { allow: boolean; role: AccessRole }): Promise<{ status: string }> {
  const result = await daemonCall<{ status: string; peer?: LanPairedPeer }>(
    'POST',
    '/api/v1/app/lan/pair-requests/' + encodeURIComponent(requestId) + '/decision',
    body,
  );
  if (result.status === 'approved' && result.peer) {
    upsertLanPeer({
      id: result.peer.id,
      name: result.peer.name,
      library_name: result.peer.library_name,
      url: normalizeLanURL(result.peer.url),
      writable: result.peer.role === 'editor',
      peer_key: result.peer.peer_key,
      last_connected_at_ms: 0,
    });
  }
  return { status: result.status };
}

export async function lanForget(peerId: string): Promise<void> {
  if (lanConn?.peer_id === peerId) {
    setLanTarget(null);
    switchToLocal();
  }
  removeStoredLanPeer(peerId);
}

export async function lanPair(input: { peer: LanPeer; writable: boolean; reverseWritable: boolean }): Promise<ConnectionDescriptor> {
  const reverse = await addReversePeerKey(input.peer, input.reverseWritable);
  let approved = false;
  try {
    const base = normalizeLanURL(input.peer.url);
    const created = await directCall<{ request_id: string; expires_in_seconds: number }>(base, '/api/v1/app/lan/pair-requests', '', 'POST', {
      client_name: os.hostname(),
      role: input.writable ? 'editor' : 'viewer',
      reverse: reverse.profile,
    });
    const requestId = created.request_id;
    const deadline = Date.now() + Math.min(Math.max(created.expires_in_seconds, 1), 95) * 1000;
    while (Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 700));
      const status = await directCall<{
        protocol: number;
        status: 'pending' | 'approved' | 'denied' | 'expired';
        access_key?: string;
        role?: AccessRole;
        peer_id?: string;
        name?: string;
        library_name?: string;
        target_library_id?: string;
      }>(base, '/api/v1/app/lan/pair-requests/' + encodeURIComponent(requestId), '', 'GET');
      if (status.protocol !== 3) {
        throw new Error('目标端配对协议版本不兼容，请将两端 hawk 升级到同一版本');
      }
      if (status.status === 'denied') {
        throw new Error('对方拒绝了配对请求');
      }
      if (status.status === 'expired') {
        throw new Error('配对请求已过期，请重新发起');
      }
      if (status.status !== 'approved') {
        continue;
      }
      if (!status.access_key) {
        throw new Error('目标设备返回了无效的访问凭证');
      }
      const info = await directCall<TargetAppInfo>(base, '/api/v1/app/info', status.access_key);
      if (!info.has_library) {
        throw new Error('目标设备未打开素材库');
      }
      const peerId = status.peer_id || input.peer.id;
      approved = true;
      upsertLanPeer({
        id: peerId,
        name: status.name || input.peer.name,
        library_name: status.library_name || input.peer.library_name,
        url: base,
        writable: status.role === 'editor',
        peer_key: status.access_key,
        last_connected_at_ms: Date.now(),
      });
      return connectLanPeer(peerId);
    }
    throw new Error('等待对方确认超时，请重新发起配对');
  } finally {
    if (!approved) {
      await removeReversePeerKey(reverse.keyId).catch(() => {});
    }
  }
}

export async function lanConnect(peerId: string): Promise<ConnectionDescriptor> {
  return connectLanPeer(peerId);
}

interface TargetLanConnectionInfo {
  protocol: number;
  peer_id: string;
  name: string;
  library_name: string;
  peer_port: number;
  target_library_id: string;
}

export async function lanConnectWithKey(input: { url: string; accessKey: string }): Promise<ConnectionDescriptor> {
  const base = normalizeLanURL(input.url);
  const accessKey = input.accessKey.trim();
  if (!accessKey) {
    throw new Error('访问密钥不能为空');
  }
  const info = await directCall<TargetLanConnectionInfo>(base, '/api/v1/app/lan/connection-info', '');
  if (info.protocol !== 3) {
    throw new Error('目标端配对协议版本不兼容，请将两端 hawk 升级到同一版本');
  }
  const appInfo = await directCall<TargetAppInfo>(base, '/api/v1/app/info', accessKey);
  if (!appInfo.has_library) {
    throw new Error('目标设备未打开素材库');
  }
  upsertLanPeer({
    id: info.peer_id,
    name: info.name,
    library_name: info.library_name,
    url: base,
    writable: appInfo.role === 'editor',
    peer_key: accessKey,
    last_connected_at_ms: Date.now(),
  });
  try {
    return await connectLanPeer(info.peer_id);
  } catch (error) {
    removeStoredLanPeer(info.peer_id);
    throw error;
  }
}

async function connectLanPeer(peerId: string): Promise<ConnectionDescriptor> {
  const profile = getStoredLanPeer(peerId);
  if (!profile) {
    throw new Error('局域网设备记录不存在，请重新配对');
  }
  upsertLanPeer({ ...profile, last_connected_at_ms: Date.now() });
  lanConn = {
    api_base: profile.url,
    credential: profile.peer_key,
    label: `${profile.name}（${profile.library_name}）`,
    remote: true,
    kind: 'lan',
    peer_id: profile.id,
  };
  setLanTarget(profile.id);
  startLanMonitor(profile);
  emitConnection(lanConn);
  return lanConn;
}

// ---------- 局域网连接健康检查 ----------

let lanMonitorTimer: ReturnType<typeof setInterval> | null = null;
let lanMonitorBusy = false;

function stopLanMonitor(): void {
  if (lanMonitorTimer) {
    clearInterval(lanMonitorTimer);
    lanMonitorTimer = null;
  }
  lanMonitorBusy = false;
  lanReconnecting = false;
}

async function checkLan(profile: NonNullable<ReturnType<typeof getStoredLanPeer>>): Promise<void> {
  if (lanMonitorBusy || lanConn?.peer_id !== profile.id) {
    return;
  }
  lanMonitorBusy = true;
  try {
    await directCall<TargetAppInfo>(profile.url, '/api/v1/app/info', profile.peer_key);
    if (lanConn?.peer_id !== profile.id) {
      return;
    }
    if (lanReconnecting) {
      lanReconnecting = false;
      upsertLanPeer({ ...profile, last_connected_at_ms: Date.now() });
      emitConnectionNotice(IPC.connectionRestored);
    }
  } catch {
    if (lanConn?.peer_id !== profile.id) {
      return;
    }
    if (!lanReconnecting) {
      lanReconnecting = true;
      emitConnectionNotice(IPC.connectionLost);
    }
  } finally {
    lanMonitorBusy = false;
  }
}

function startLanMonitor(profile: ReturnType<typeof getStoredLanPeer>): void {
  stopLanMonitor();
  if (!profile) {
    return;
  }
  // 固定每 10s 检查；断线后保留目标继续重试，直到成功或用户显式切回本地。
  lanMonitorTimer = setInterval(() => {
    void checkLan(profile);
  }, LAN_MONITOR_INTERVAL_MS);
}

export async function restoreLanSession(): Promise<ConnectionDescriptor | null> {
  const target = getLanTarget();
  if (!target || !getStoredLanPeer(target)) {
    return null;
  }
  restoring = true;
  try {
    return await connectLanPeer(target);
  } catch {
    return null;
  } finally {
    restoring = false;
  }
}

/** 断开当前局域网连接并回切本地库（幂等）。 */
export async function disconnectCurrent(): Promise<ConnectionDescriptor | null> {
  setLanTarget(null);
  switchToLocal();
  return currentDescriptor();
}

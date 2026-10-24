// 远程访问接线枢纽：连接描述符（当前活动连接）+ 本地 daemon SSE 订阅（remote.session 断线回切）。
// 渲染进程切到远程后其 API base 已指向会话代理，远程管理面（状态/设备/会话）必须留在主进程：
// 主进程始终持有本地 daemon 的地址与 admin token，经正规 REST 调用（不经 IPC 传业务数据之外的状态）。
import os from 'node:os';
import { randomBytes } from 'node:crypto';
import { lanAddresses } from './lan';
import { getStartedConn } from './server';
import { getMainWindow } from './window';
import { getLanTarget, getRemoteTarget, getStoredLanPeer, listLanPeers, removeStoredLanPeer, setLanTarget, setRemoteTarget, upsertLanPeer } from './app-config';
import {
  IPC,
  type ConnectionDescriptor,
  type LanClient,
  type LanPairRequest,
  type LanPeer,
  type LanPeerProfile,
  type RemoteConfig,
  type RemoteDevice,
  type RemoteDto,
  type RemoteKeyCreated,
  type RemoteKeyEntry,
  type RemoteSessionDto,
} from './ipc-contract';

/** 当前远程连接（null = 本地库）。渲染进程拿到的是完整描述符（api_base + token + label） */
let remoteConn: ConnectionDescriptor | null = null;
/** 当前局域网直连（不经本机 daemon 会话代理；token 是目标 LAN web 的 viewer/write token） */
let lanConn: ConnectionDescriptor | null = null;
/** 冷启动恢复远端库进行中：期间不外泄本地库描述符（渲染进程挂载时的拉取兜底
 *  不得先注水本地库，否则会闪一下本地库再切远端） */
let restoring = false;
/** SSE 订阅的中止开关（daemon 重启/换库时废弃重连） */
let sseAbort: AbortController | null = null;

/** 本地库描述符（label 恒为「我的库」） */
function localDescriptor(): ConnectionDescriptor | null {
  const conn = getStartedConn();
  return conn ? { api_base: conn.address, token: conn.token, label: '我的库', remote: false, kind: 'local' } : null;
}

/** 当前活动连接描述符：远程/局域网直连优先，其次本地库 */
export function currentDescriptor(): ConnectionDescriptor | null {
  return remoteConn ?? lanConn ?? (restoring ? null : localDescriptor());
}

/** 推送连接变更：渲染进程统一执行切换重置（abort 请求/SSE → 清 store → 重新注水） */
function emitConnection(descriptor: ConnectionDescriptor): void {
  getMainWindow()?.webContents.send(IPC.connectionChanged, descriptor);
}

/** 回切本地库（主动断开与断线回切共用）；本就本地时为空操作 */
function switchToLocal(): void {
  if (remoteConn === null && lanConn === null) {
    return;
  }
  remoteConn = null;
  lanConn = null;
  stopLanMonitor();
  const local = localDescriptor();
  if (local) {
    emitConnection(local);
  }
}

/** server 就绪（startedConn 写入后调用）：重建 SSE 订阅（remote.session 断线回切依赖） */
export function onServerReady(): void {
  sseAbort?.abort();
  const conn = getStartedConn();
  if (!conn) {
    return;
  }
  sseAbort = watchRemoteSessionEvents(conn.address, conn.token);
}

/** server 停止/换库：远程会话随之消亡，回切本地并废弃 SSE */
export function onServerStopped(): void {
  sseAbort?.abort();
  sseAbort = null;
  switchToLocal();
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
      Authorization: `Bearer ${conn.token}`,
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

// ---------- IPC 暴露的操作（渲染进程远程设置面板与设备列表驱动） ----------

export async function remoteStatus(): Promise<RemoteDto> {
  return daemonCall<RemoteDto>('GET', '/api/v1/remote');
}

export async function remotePutConfig(config: RemoteConfig): Promise<RemoteDto> {
  return daemonCall<RemoteDto>('PUT', '/api/v1/remote', config);
}

export async function remoteDevices(): Promise<{ devices: RemoteDevice[] }> {
  return daemonCall<{ devices: RemoteDevice[] }>('GET', '/api/v1/remote/devices');
}

export async function remotePatchDevice(patch: { name?: string; allow_remote?: boolean; library_name?: string }): Promise<RemoteDevice> {
  return daemonCall<RemoteDevice>('PATCH', '/api/v1/remote/device', patch);
}

/** 注销服务端设备记录（本机当前设备由 daemon 侧拒绝）；其进行中会话与 key 映射一并清理 */
export async function remoteDeleteDevice(deviceId: string): Promise<void> {
  await daemonCall('DELETE', `/api/v1/remote/devices/${encodeURIComponent(deviceId)}`);
}

export async function remoteRotate(): Promise<{ endpoint_id: string }> {
  return daemonCall<{ endpoint_id: string }>('POST', '/api/v1/remote/device/rotate');
}

// ---------- 访问密钥（hawk-app key）管理：透传 daemon（admin token 不出主进程） ----------

export async function remoteKeys(): Promise<{ keys: RemoteKeyEntry[] }> {
  return daemonCall<{ keys: RemoteKeyEntry[] }>('GET', '/api/v1/remote/keys');
}

export async function remoteCreateKey(input: { label?: string; read: boolean; write?: boolean; custom?: string }): Promise<RemoteKeyCreated> {
  return daemonCall<RemoteKeyCreated>('POST', '/api/v1/remote/keys', input);
}

export async function remotePatchKey(
  id: string,
  patch: { label?: string; read?: boolean; write?: boolean },
): Promise<{ key: RemoteKeyEntry; synced: boolean }> {
  return daemonCall<{ key: RemoteKeyEntry; synced: boolean }>('PATCH', `/api/v1/remote/keys/${encodeURIComponent(id)}`, patch);
}

export async function remoteDeleteKey(id: string): Promise<{ synced: boolean }> {
  return daemonCall<{ synced: boolean }>('DELETE', `/api/v1/remote/keys/${encodeURIComponent(id)}`);
}

/** 建立远程会话并切换连接描述符；失败抛错（message 直接展示）。
 *  直连/打洞/中继由传输层自治（是否用中继由两端静态开关决定，无逐次确认） */
export async function remoteConnect(targetDeviceId: string): Promise<ConnectionDescriptor> {
  const session = await daemonCall<RemoteSessionDto>('POST', '/api/v1/remote/session', {
    target_device_id: targetDeviceId,
  });
  lanConn = null;
  stopLanMonitor();
  setLanTarget(null);
  remoteConn = {
    api_base: `http://127.0.0.1:${session.proxy_port}`,
    token: session.token,
    label: session.target_name,
    remote: true,
    kind: 'remote',
    device_id: session.target_device_id,
  };
  // 记住远端目标：下次启动（daemon 就绪后）直接尝试恢复同一远端库
  setRemoteTarget(session.target_device_id);
  emitConnection(remoteConn);
  return remoteConn;
}

/** 断开当前远端/局域网连接并回切本地库（幂等）。显式断开即忘记自动恢复目标 */
export async function remoteDisconnect(): Promise<ConnectionDescriptor | null> {
  try {
    if (remoteConn) {
      await daemonCall('DELETE', '/api/v1/remote/session');
    }
  } finally {
    setRemoteTarget(null);
    setLanTarget(null);
    switchToLocal();
  }
  return currentDescriptor();
}

// ---------- 局域网直连（不经 hawk-server） ----------

/** 直连目标的普通 HTTP 调用：目标 LAN web 已开 CORS，Bearer token 走 viewer/write token */
async function directCall<T>(base: string, path: string, token: string, method = 'GET', body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${base}${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
      signal: AbortSignal.timeout(6000),
    });
  } catch {
    throw new Error('无法连接局域网设备（确认在同一网络且目标已启用局域网查看）');
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

/** 目标库 app/info 的最小校验（连接前确认 token 有效且确为素材库） */
interface TargetAppInfo {
  has_library: boolean;
  access: 'viewer' | 'admin' | 'remote_writer';
  writable: boolean;
}

function normalizeLanURL(url: string): string {
  const parsed = new URL(url);
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    throw new Error('局域网设备地址必须使用 http/https');
  }
  return parsed.toString().replace(/\/+$/, '');
}

/** 探测同网段可配对的 hawk 设备（本机 daemon 发 UDP 组播/广播） */
export async function lanDiscover(): Promise<{ peers: LanPeer[] }> {
  return daemonCall<{ peers: LanPeer[] }>('GET', '/api/v1/app/lan/peers');
}

/** 已配对设备列表（剥掉 token） */
export async function lanPeers(): Promise<{ peers: LanPeerProfile[] }> {
  return { peers: listLanPeers() };
}

/** 当前局域网连接延迟：1 次预热 + 5 次 app/info 往返，取中位数毫秒 */
export async function lanSpeedTest(): Promise<{ latency_ms: number }> {
  const peerId = lanConn?.peer_id;
  if (!peerId) {
    throw new Error('当前未连接局域网设备');
  }
  const profile = getStoredLanPeer(peerId);
  if (!profile) {
    throw new Error('局域网设备记录不存在，请重新配对');
  }
  await directCall<TargetAppInfo>(profile.url, '/api/v1/app/info', profile.token);
  const samples: number[] = [];
  for (let i = 0; i < 5; i += 1) {
    const started = Date.now();
    await directCall<TargetAppInfo>(profile.url, '/api/v1/app/info', profile.token);
    samples.push(Date.now() - started);
  }
  samples.sort((a, b) => a - b);
  return { latency_ms: samples[Math.floor(samples.length / 2)] };
}

interface LanKeyDto {
  id: string;
  label: string;
  token: string;
  write: boolean;
  created_at_ms: number;
}

interface LanSettingsDto {
  enabled: boolean;
  port: number;
  peer_port: number;
  peer_id: string;
  name: string;
  library_name: string;
  keys: LanKeyDto[];
  peer_keys: LanKeyDto[];
  active: boolean;
  peer_active: boolean;
}

interface ReversePeerProfile {
  peer_id: string;
  name: string;
  library_name: string;
  urls: string[];
  token: string;
  writable: boolean;
}

interface LanPairedPeer {
  id: string;
  name: string;
  library_name: string;
  url: string;
  writable: boolean;
  token: string;
}
/** 源端发起配对前在本机签一把反向密钥，目标端批准后即可反向连接源端。 */
async function addReversePeerKey(target: LanPeer): Promise<{ keyId: string; profile: ReversePeerProfile }> {
  const settings = await daemonCall<LanSettingsDto>('GET', '/api/v1/app/lan');
  const token = randomBytes(32).toString('hex');
  const keyId = 'peer_' + randomBytes(8).toString('hex');
  const updated = await daemonCall<LanSettingsDto>('PUT', '/api/v1/app/lan', {
    enabled: settings.enabled,
    port: settings.port,
    peer_port: settings.peer_port,
    keys: settings.keys,
    peer_keys: [...settings.peer_keys, { id: keyId, label: '局域网设备：' + target.name, token, write: false, created_at_ms: Date.now() }],
  });
  return {
    keyId,
    profile: {
      peer_id: updated.peer_id,
      name: updated.name,
      library_name: updated.library_name,
      urls: lanAddresses().map((ip) => 'http://' + ip + ':' + updated.peer_port),
      token,
      writable: false,
    },
  };
}

/** 配对失败/被拒时清理源端为反向连接临时签发的密钥。 */
async function removeReversePeerKey(keyId: string): Promise<void> {
  const settings = await daemonCall<LanSettingsDto>('GET', '/api/v1/app/lan');
  const peerKeys = settings.peer_keys.filter((key) => key.id !== keyId);
  if (peerKeys.length === settings.peer_keys.length) {
    return;
  }
  await daemonCall<LanSettingsDto>('PUT', '/api/v1/app/lan', {
    enabled: settings.enabled,
    port: settings.port,
    peer_port: settings.peer_port,
    keys: settings.keys,
    peer_keys: peerKeys,
  });
}
/** 本机 daemon 上待确认的自动配对请求（设置/全局提示均经主进程读取）。 */
export async function lanPairRequests(): Promise<{ requests: LanPairRequest[] }> {
  return daemonCall<{ requests: LanPairRequest[] }>('GET', '/api/v1/app/lan/pair-requests');
}

/** 本机 daemon 当前活跃的直连设备（目标端显示谁正在连接）。 */
export async function lanClients(): Promise<{ clients: LanClient[] }> {
  return daemonCall<{ clients: LanClient[] }>('GET', '/api/v1/app/lan/clients');
}

/** 目标端用户确认/拒绝配对请求；批准时把源端回连信息保存为已配对设备。 */
export async function lanDecidePairRequest(requestId: string, body: { allow: boolean; write: boolean }): Promise<{ status: string }> {
  const result = await daemonCall<{ status: string; peer?: LanPairedPeer }>(
    'POST',
    '/api/v1/app/lan/pair-requests/' + encodeURIComponent(requestId) + '/decision',
    body,
  );
  if (result.status === 'approved' && result.peer) {
    try {
      upsertLanPeer({
        id: result.peer.id,
        name: result.peer.name,
        library_name: result.peer.library_name,
        url: normalizeLanURL(result.peer.url),
        writable: result.peer.writable,
        token: result.peer.token,
        last_connected_at_ms: 0,
      });
    } catch {
      // 源端地址异常时不阻断本次审批；单向后仍可正常使用。
    }
  }
  return { status: result.status };
}

/** 忘记一台已配对设备（若正在浏览则先回本地库） */
export async function lanForget(peerId: string): Promise<void> {
  if (lanConn?.peer_id === peerId) {
    setLanTarget(null);
    switchToLocal();
  }
  removeStoredLanPeer(peerId);
}

/** 向目标设备发起自动配对请求；目标端批准后双方各自保存一台可反向连接的设备。 */
export async function lanPair(input: { peer: LanPeer; writable: boolean }): Promise<ConnectionDescriptor> {
  const reverse = await addReversePeerKey(input.peer).catch(() => null);
  let approved = false;
  try {
    const base = normalizeLanURL(input.peer.url);
    const created = await directCall<{ request_id: string; expires_in_seconds: number }>(base, '/api/v1/app/lan/pair-requests', '', 'POST', {
      client_name: os.hostname(),
      writable: input.writable,
      reverse: reverse?.profile,
    });
    const requestId = created.request_id;
    const deadline = Date.now() + Math.min(Math.max(created.expires_in_seconds, 1), 95) * 1000;
    while (Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 700));
      const status = await directCall<{
        status: 'pending' | 'approved' | 'denied' | 'expired';
        token?: string;
        writable?: boolean;
        peer_id?: string;
        name?: string;
        library_name?: string;
      }>(base, '/api/v1/app/lan/pair-requests/' + encodeURIComponent(requestId), '', 'GET');
      if (status.status === 'denied') {
        throw new Error('对方拒绝了配对请求');
      }
      if (status.status === 'expired') {
        throw new Error('配对请求已过期，请重新发起');
      }
      if (status.status !== 'approved') {
        continue;
      }
      if (!status.token) {
        throw new Error('目标设备返回了无效的访问凭证');
      }
      const info = await directCall<TargetAppInfo>(base, '/api/v1/app/info', status.token);
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
        writable: status.writable ?? false,
        token: status.token,
        last_connected_at_ms: Date.now(),
      });
      return connectLanPeer(peerId);
    }
    throw new Error('等待对方确认超时，请重新发起配对');
  } finally {
    if (!approved && reverse) {
      await removeReversePeerKey(reverse.keyId).catch(() => {});
    }
  }
}
/** 读取已配对设备并切换当前连接描述符 */
export async function lanConnect(peerId: string): Promise<ConnectionDescriptor> {
  return connectLanPeer(peerId);
}

async function connectLanPeer(peerId: string): Promise<ConnectionDescriptor> {
  const profile = getStoredLanPeer(peerId);
  if (!profile) {
    throw new Error('局域网设备记录不存在，请重新配对');
  }
  const info = await directCall<TargetAppInfo>(profile.url, '/api/v1/app/info', profile.token);
  if (!info.has_library) {
    throw new Error('目标设备未打开素材库');
  }
  // 切到局域网直连前，结束本地 daemon 上可能存在的远程会话（失败不阻断）
  if (remoteConn) {
    try {
      await daemonCall('DELETE', '/api/v1/remote/session');
    } catch {
      // 已断开的远程会话无需处理
    }
  }
  setRemoteTarget(null);
  remoteConn = null;
  upsertLanPeer({ ...profile, last_connected_at_ms: Date.now() });
  lanConn = {
    api_base: profile.url,
    token: profile.token,
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

// ---------- 局域网连接健康检查：目标关闭/网络切换时无声回本地库 ----------
let lanMonitorTimer: ReturnType<typeof setInterval> | null = null;
let lanMonitorFailures = 0;

function stopLanMonitor(): void {
  if (lanMonitorTimer) {
    clearInterval(lanMonitorTimer);
    lanMonitorTimer = null;
  }
  lanMonitorFailures = 0;
}

function startLanMonitor(profile: ReturnType<typeof getStoredLanPeer>): void {
  stopLanMonitor();
  if (!profile) {
    return;
  }
  lanMonitorTimer = setInterval(() => {
    void (async () => {
      try {
        await directCall<TargetAppInfo>(profile.url, '/api/v1/app/info', profile.token);
        lanMonitorFailures = 0;
      } catch {
        lanMonitorFailures += 1;
        // 留一次重试给目标短暂重启/休眠；连续失败才回切，避免 wifi 抖动时反复闪断
        if (lanMonitorFailures >= 2 && lanConn?.peer_id === profile.id) {
          setLanTarget(null);
          switchToLocal();
        }
      }
    })();
  }, 5000);
}

// ---------- 冷启动恢复上次浏览的远端库 ----------

/** 等信令 online 的预算：库就绪瞬间远程接入可能还在建数据面，此时发起会话必然失败 */
const RESTORE_ONLINE_WAIT_MS = 5000;
const RESTORE_ONLINE_POLL_MS = 250;

/** 信令是否已 online（接入是异步过程：connecting → online/error；未启用直接放弃） */
async function waitRemoteOnline(): Promise<boolean> {
  const deadline = Date.now() + RESTORE_ONLINE_WAIT_MS;
  for (;;) {
    let dto: RemoteDto;
    try {
      dto = await remoteStatus();
    } catch {
      return false; // daemon 未就绪/本地异常：不恢复
    }
    if (!dto.enabled) {
      return false;
    }
    if (dto.status.state === 'online') {
      return true;
    }
    if (dto.status.state === 'error' || Date.now() >= deadline) {
      return false;
    }
    await new Promise((resolve) => setTimeout(resolve, RESTORE_ONLINE_POLL_MS));
  }
}

/**
 * 冷启动恢复上次浏览的远端库（daemon 就绪后、通知渲染进程之前调用）：成功后当前连接描述符
 * 已是远端（connection-changed 由 remoteConnect 推送），渲染进程首个注水即远端库。
 * 无记录/接入未启用/目标离线或关闭访问/会话建不起来一律返回 null（调用方回退本地库）；
 * 目标记录不清除——设备暂时离线不代表用户不再看这个库，下次启动继续尝试。
 */
export async function restoreRemoteSession(): Promise<ConnectionDescriptor | null> {
  const target = getRemoteTarget();
  if (!target) {
    return null;
  }
  restoring = true;
  try {
    if (!(await waitRemoteOnline())) {
      return null;
    }
    // 目标离线/关闭远程访问：不必发起会话（服务端也只会等到超时）
    try {
      const { devices } = await remoteDevices();
      const device = devices.find((d) => d.device_id === target);
      if (!device?.online || !device.allow_remote) {
        return null;
      }
    } catch {
      return null;
    }
    try {
      return await remoteConnect(target);
    } catch {
      // 隧道/服务端失败：回退本地库，记录保留（下次启动再试）
      return null;
    }
  } finally {
    restoring = false;
  }
}

/** 冷启动恢复上次浏览的局域网设备；失败回 null（记录保留，下次启动再试） */
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

// ---------- SSE：remote.session 断线回切 ----------

/** 订阅本地 daemon 事件流，remote.session disconnected → 自动回切本地库。
 *  断线重连：指数退避（daemon 重启场景由 onServerReady 整体重建，此处只管网络闪断） */
function watchRemoteSessionEvents(address: string, token: string): AbortController {
  const abort = new AbortController();
  void (async () => {
    let retryMs = 1000;
    while (!abort.signal.aborted) {
      try {
        const res = await fetch(`${address}/api/v1/events?token=${encodeURIComponent(token)}`, {
          signal: abort.signal,
        });
        if (!res.ok || !res.body) {
          throw new Error(`HTTP ${res.status}`);
        }
        retryMs = 1000;
        await consumeEventStream(res.body, (eventName, data) => {
          if (eventName !== 'remote.session' || remoteConn === null) {
            return;
          }
          try {
            const payload = JSON.parse(data) as { state?: string };
            if (payload.state === 'disconnected') {
              // 隧道断开/对端离线：无声回切本地库（用户看到的是回到自己的库，而非远程白屏报错）
              switchToLocal();
            }
          } catch {
            // 载荷解析失败忽略
          }
        });
      } catch {
        if (abort.signal.aborted) {
          return;
        }
      }
      await new Promise((resolve) => setTimeout(resolve, retryMs));
      retryMs = Math.min(retryMs * 2, 15_000);
    }
  })();
  return abort;
}

/** 逐行解析 SSE 帧（event:/data: 两行式），空行分发一帧 */
async function consumeEventStream(body: ReadableStream<Uint8Array>, onEvent: (event: string, data: string) => void): Promise<void> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  let eventName = '';
  let data = '';
  for (;;) {
    const { done, value } = await reader.read();
    if (done) {
      return;
    }
    buffer += decoder.decode(value, { stream: true });
    let newline: number;
    while ((newline = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, newline).replace(/\r$/, '');
      buffer = buffer.slice(newline + 1);
      if (line === '') {
        if (eventName && data) {
          onEvent(eventName, data);
        }
        eventName = '';
        data = '';
      } else if (line.startsWith('event: ')) {
        eventName = line.slice(7);
      } else if (line.startsWith('data: ')) {
        data = line.slice(6);
      }
    }
  }
}

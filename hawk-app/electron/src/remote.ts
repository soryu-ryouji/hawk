// 远程访问接线枢纽：连接描述符（当前活动连接）+ 本地 daemon SSE 订阅（remote.session 断线回切）。
// 渲染进程切到远程后其 API base 已指向会话代理，远程管理面（状态/设备/会话）必须留在主进程：
// 主进程始终持有本地 daemon 的地址与 admin token，经正规 REST 调用（不经 IPC 传业务数据之外的状态）。
import { getStartedConn } from './server';
import { getMainWindow } from './window';
import {
  IPC,
  type ConnectionDescriptor,
  type RemoteConfig,
  type RemoteDevice,
  type RemoteDto,
  type RemoteKeyCreated,
  type RemoteKeyEntry,
  type RemoteSessionDto,
} from './ipc-contract';

/** 当前远程连接（null = 本地库）。渲染进程拿到的是完整描述符（api_base + token + label） */
let remoteConn: ConnectionDescriptor | null = null;
/** SSE 订阅的中止开关（daemon 重启/换库时废弃重连） */
let sseAbort: AbortController | null = null;

/** 本地库描述符（label 恒为「我的库」） */
function localDescriptor(): ConnectionDescriptor | null {
  const conn = getStartedConn();
  return conn ? { api_base: conn.address, token: conn.token, label: '我的库', remote: false } : null;
}

/** 当前活动连接描述符：远程会话代理优先，其次本地库 */
export function currentDescriptor(): ConnectionDescriptor | null {
  return remoteConn ?? localDescriptor();
}

/** 推送连接变更：渲染进程统一执行切换重置（abort 请求/SSE → 清 store → 重新注水） */
function emitConnection(descriptor: ConnectionDescriptor): void {
  getMainWindow()?.webContents.send(IPC.connectionChanged, descriptor);
}

/** 回切本地库（主动断开与断线回切共用）；本就本地时为空操作 */
function switchToLocal(): void {
  if (remoteConn === null) {
    return;
  }
  remoteConn = null;
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
  remoteConn = {
    api_base: `http://127.0.0.1:${session.proxy_port}`,
    token: session.token,
    label: session.target_name,
    remote: true,
    device_id: session.target_device_id,
  };
  emitConnection(remoteConn);
  return remoteConn;
}

/** 断开远程会话并回切本地库（幂等） */
export async function remoteDisconnect(): Promise<ConnectionDescriptor | null> {
  try {
    await daemonCall('DELETE', '/api/v1/remote/session');
  } finally {
    switchToLocal();
  }
  return currentDescriptor();
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

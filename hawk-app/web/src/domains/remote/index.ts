// 远程访问域：设置面板的状态源（接入配置/信令状态/设备列表/会话动作）。
// 连接切换本身由主进程驱动（remoteConnect → connection-changed → startup 重注水），
// 本 store 只管面板数据与动作，不持连接语义。
import { computed, ref } from 'vue';
import { defineStore } from 'pinia';

export { initConnectionLabel, useConnectionLabel } from './connection';
import { shell } from '@/shared/lib/platform';
import type { RemoteConfig, RemoteDevice, RemoteDto, RemoteKeyCreated, RemoteKeyEntry } from '@/shared/types';

/** IPC invoke 错误消息剥壳：'Error invoking remote method ...: Error: <message>' → <message> */
export function ipcMessage(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  return raw.replace(/^Error invoking remote method '[^']+': (Error: )?/, '');
}

export const useRemoteStore = defineStore('remote', () => {
  const dto = ref<RemoteDto | null>(null);
  const devices = ref<RemoteDevice[]>([]);
  /** 本机签发的访问密钥（hawk-app key，不含明文） */
  const keys = ref<RemoteKeyEntry[]>([]);
  const loading = ref(false);
  const saving = ref(false);
  /** 正在建立会话的设备 id（列表行内 spinner） */
  const connectingId = ref<string | null>(null);
  const error = ref<string | null>(null);
  /** 「使用中继」开关（设备级，daemon 远程配置 relay；默认关闭：中继消耗自己的服务器带宽）。
   *  传输路径由 iroh 自治：开关只决定本机是否使用中继，无逐次确认 */
  const relayEnabled = computed(() => dto.value?.relay ?? false);

  let pollTimer = 0;

  /** 开关随配置写回 daemon（dto 即回显；未拉到配置时不动作） */
  async function setRelayEnabled(on: boolean): Promise<void> {
    const current = dto.value;
    if (!current) {
      return;
    }
    await saveConfig({
      enabled: current.enabled,
      url: current.url,
      service_key: current.service_key,
      writable: current.writable,
      relay: on,
    });
  }

  async function refresh(): Promise<void> {
    try {
      dto.value = await shell.remoteStatus();
      error.value = null;
    } catch (e) {
      error.value = ipcMessage(e);
    }
  }

  async function refreshDevices(): Promise<void> {
    if (!dto.value?.enabled || dto.value.status.state !== 'online') {
      devices.value = [];
      return;
    }
    try {
      devices.value = (await shell.remoteDevices()).devices;
    } catch (e) {
      error.value = ipcMessage(e);
    }
  }

  /** 访问密钥列表。密钥由本地 daemon 持久保管（remote/keys.json），与是否接入无关——
   *  不得以 dto 状态为门禁（挂载时 dto 未就绪会把列表清空且不再补拉，造成已签发密钥不可见） */
  async function refreshKeys(): Promise<void> {
    try {
      keys.value = (await shell.remoteKeys()).keys;
    } catch (e) {
      error.value = ipcMessage(e);
    }
  }

  /** 签发访问密钥：返回明文（只在本次返回值可见）；失败返回 null（error 已填） */
  async function createKey(input: { label?: string; read: boolean; write?: boolean; custom?: string }): Promise<RemoteKeyCreated | null> {
    error.value = null;
    try {
      const created = await shell.remoteCreateKey(input);
      await refreshKeys();
      return created;
    } catch (e) {
      error.value = ipcMessage(e);
      return null;
    }
  }

  async function patchKey(id: string, patch: { label?: string; read?: boolean; write?: boolean }): Promise<boolean> {
    try {
      await shell.remotePatchKey(id, patch);
      await refreshKeys();
      return true;
    } catch (e) {
      error.value = ipcMessage(e);
      return false;
    }
  }

  async function deleteKey(id: string): Promise<boolean> {
    try {
      await shell.remoteDeleteKey(id);
      await refreshKeys();
      return true;
    } catch (e) {
      error.value = ipcMessage(e);
      return false;
    }
  }

  /** 保存接入配置（热重启信令；接入进展经轮询状态可见） */
  async function saveConfig(config: RemoteConfig): Promise<boolean> {
    saving.value = true;
    error.value = null;
    try {
      dto.value = await shell.remotePutConfig(config);
      return true;
    } catch (e) {
      error.value = ipcMessage(e);
      return false;
    } finally {
      saving.value = false;
    }
  }

  /** 建立到目标设备的远程会话（成功后主进程推 connection-changed，前端无感切换）。
   *  直连/打洞/中继由传输层自治；是否使用中继由两端静态开关决定（无逐次确认） */
  async function connect(deviceId: string): Promise<void> {
    connectingId.value = deviceId;
    error.value = null;
    try {
      await shell.remoteConnect(deviceId);
    } catch (e) {
      const message = ipcMessage(e);
      // 中继开关关闭时的不可达错误附带开启引导
      error.value = relayEnabled.value ? message : `${message}（可在远程设置中开启「使用中继」）`;
    } finally {
      connectingId.value = null;
    }
  }

  async function disconnect(): Promise<void> {
    await shell.remoteDisconnect();
  }

  /** 注销服务端设备记录（本机当前设备由 daemon 拒绝）；成功后就地刷新列表 */
  async function deleteDevice(deviceId: string): Promise<boolean> {
    try {
      await shell.remoteDeleteDevice(deviceId);
      await refreshDevices();
      return true;
    } catch (e) {
      error.value = ipcMessage(e);
      return false;
    }
  }

  async function patchDevice(patch: { name?: string; allow_remote?: boolean }): Promise<boolean> {
    try {
      await shell.remotePatchDevice(patch);
      await refreshDevices();
      return true;
    } catch (e) {
      error.value = ipcMessage(e);
      return false;
    }
  }

  async function rotate(): Promise<boolean> {
    try {
      await shell.remoteRotate();
      return true;
    } catch (e) {
      error.value = ipcMessage(e);
      return false;
    }
  }

  /** 面板打开期间轮询：信令状态 3s、设备列表 5s（打开/关闭由组件驱动） */
  function startPolling(): void {
    stopPolling();
    loading.value = true;
    void Promise.all([refresh(), refreshDevices()]).finally(() => {
      loading.value = false;
    });
    pollTimer = window.setInterval(() => {
      void refresh().then(refreshDevices);
    }, 3000);
  }

  function stopPolling(): void {
    clearInterval(pollTimer);
    pollTimer = 0;
  }

  return {
    dto,
    devices,
    keys,
    loading,
    saving,
    connectingId,
    error,
    relayEnabled,
    setRelayEnabled,
    refresh,
    refreshDevices,
    refreshKeys,
    createKey,
    patchKey,
    deleteKey,
    saveConfig,
    connect,
    disconnect,
    deleteDevice,
    patchDevice,
    rotate,
    startPolling,
    stopPolling,
  };
});

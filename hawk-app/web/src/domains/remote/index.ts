// 远程访问域：设置面板的状态源（接入配置/信令状态/设备列表/会话动作）。
// 连接切换本身由主进程驱动（remoteConnect → connection-changed → startup 重注水），
// 本 store 只管面板数据与动作，不持连接语义。
import { ref } from 'vue';
import { defineStore } from 'pinia';
import { loadText, saveText, STORAGE_KEYS } from '@/shared/lib/persist';

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
  /** 「中继回退」开关（localStorage 持久化，默认关闭：中继消耗自己的服务器带宽，须用户主动开启） */
  const relayFallback = ref(loadText(STORAGE_KEYS.relayFallback) === '1');

  let pollTimer = 0;

  function setRelayFallback(on: boolean): void {
    relayFallback.value = on;
    saveText(STORAGE_KEYS.relayFallback, on ? '1' : '0');
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

  /** 访问密钥列表（远程未接入时清空——key 只有接入后才有意义） */
  async function refreshKeys(): Promise<void> {
    if (!dto.value?.enabled) {
      keys.value = [];
      return;
    }
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
   *  直连失败且「中继回退」已开启时，弹窗询问用户是否经服务端中继继续（不自动回落） */
  async function connect(deviceId: string): Promise<void> {
    connectingId.value = deviceId;
    error.value = null;
    try {
      await shell.remoteConnect(deviceId, 'direct');
    } catch (e) {
      const directError = ipcMessage(e);
      if (relayFallback.value && window.confirm(`直连失败：${directError}\n\n是否经服务端中继继续？（素材流量将经过你的服务器带宽，内容仍为端到端加密）`)) {
        try {
          await shell.remoteConnect(deviceId, 'relay');
        } catch (e2) {
          error.value = ipcMessage(e2);
        }
      } else if (!relayFallback.value) {
        error.value = `${directError}（可在远程设置中开启「中继回退」）`;
      } else {
        error.value = directError;
      }
    } finally {
      connectingId.value = null;
    }
  }

  async function disconnect(): Promise<void> {
    await shell.remoteDisconnect();
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
    relayFallback,
    setRelayFallback,
    refresh,
    refreshDevices,
    refreshKeys,
    createKey,
    patchKey,
    deleteKey,
    saveConfig,
    connect,
    disconnect,
    patchDevice,
    rotate,
    startPolling,
    stopPolling,
  };
});

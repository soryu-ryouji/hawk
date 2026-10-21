// 远程访问域：设置面板的状态源（接入配置/信令状态/设备列表/会话动作）。
// 连接切换本身由主进程驱动（remoteConnect → connection-changed → startup 重注水），
// 本 store 只管面板数据与动作，不持连接语义。
import { ref } from 'vue';
import { defineStore } from 'pinia';

export { initConnectionLabel, useConnectionLabel } from './connection';
import { shell } from '@/shared/lib/platform';
import type { RemoteConfig, RemoteDevice, RemoteDto } from '@/shared/types';

/** IPC invoke 错误消息剥壳：'Error invoking remote method ...: Error: <message>' → <message> */
export function ipcMessage(e: unknown): string {
  const raw = e instanceof Error ? e.message : String(e);
  return raw.replace(/^Error invoking remote method '[^']+': (Error: )?/, '');
}

export const useRemoteStore = defineStore('remote', () => {
  const dto = ref<RemoteDto | null>(null);
  const devices = ref<RemoteDevice[]>([]);
  const loading = ref(false);
  const saving = ref(false);
  /** 正在建立会话的设备 id（列表行内 spinner） */
  const connectingId = ref<string | null>(null);
  const error = ref<string | null>(null);

  let pollTimer = 0;

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

  /** 建立到目标设备的远程会话（成功后主进程推 connection-changed，前端无感切换） */
  async function connect(deviceId: string): Promise<void> {
    connectingId.value = deviceId;
    error.value = null;
    try {
      await shell.remoteConnect(deviceId);
    } catch (e) {
      error.value = ipcMessage(e);
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
    loading,
    saving,
    connectingId,
    error,
    refresh,
    refreshDevices,
    saveConfig,
    connect,
    disconnect,
    patchDevice,
    rotate,
    startPolling,
    stopPolling,
  };
});

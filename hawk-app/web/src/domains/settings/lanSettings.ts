import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '@/shared/api/endpoints';
import { errorText } from '@/shared/lib/storeUtil';
import type { AccessRole } from '@/shared/types';

export interface LanAccessKey {
  id: string;
  label: string;
  access_key: string;
  role: AccessRole;
  created_at_ms: number;
  peer_id?: string | null;
  library_name?: string | null;
  url?: string | null;
}

export const useLanSettingsStore = defineStore('lanSettings', () => {
  const loaded = ref(false);
  const loading = ref(false);
  const saving = ref(false);
  const error = ref<string | null>(null);
  const enabled = ref(false);
  const port = ref('27372');
  const peerPort = ref('27375');
  const keys = ref<LanAccessKey[]>([]);
  const peerKeys = ref<LanAccessKey[]>([]);
  const active = ref(false);
  const peerActive = ref(false);
  const peerError = ref<string | null>(null);
  const targetIdentity = ref('');
  const tlsFingerprint = ref<string | undefined>(undefined);
  const targetDeviceId = ref<string | undefined>(undefined);
  const targetLibraryId = ref('');

  const portValid = computed(() => validPort(port.value));
  const peerPortValid = computed(() => validPort(peerPort.value));
  const PORT_ERROR = '端口须为 1–65535 之间的数字';

  function validPort(value: string): boolean {
    return /^\d+$/.test(value.trim()) && Number(value) >= 1 && Number(value) <= 65535;
  }

  function apply(dto: {
    enabled: boolean;
    port: number;
    peer_port: number;
    keys: LanAccessKey[];
    peer_keys: LanAccessKey[];
    active: boolean;
    peer_active: boolean;
    peer_error?: string | null;
    target_identity: string;
    tls_fingerprint?: string | null;
    target_device_id?: string | null;
    target_library_id: string;
  }): void {
    enabled.value = dto.enabled;
    port.value = String(dto.port);
    peerPort.value = String(dto.peer_port);
    keys.value = dto.keys.map((key) => ({ ...key }));
    // LAN Web 与原生 Peer 已统一为同一 AccessKey；peer_keys 仅为旧 DTO 字段。
    peerKeys.value = dto.keys.map((key) => ({ ...key }));
    active.value = dto.active;
    peerActive.value = dto.peer_active;
    peerError.value = dto.peer_error ?? null;
    targetIdentity.value = dto.target_identity;
    tlsFingerprint.value = dto.tls_fingerprint ?? undefined;
    targetDeviceId.value = dto.target_device_id ?? undefined;
    targetLibraryId.value = dto.target_library_id;
  }

  async function load(force = false): Promise<void> {
    if (loaded.value && !force) {
      return;
    }
    loading.value = true;
    error.value = null;
    try {
      apply(await api.appLan());
      loaded.value = true;
    } catch (e) {
      error.value = `读取设置失败：${errorText(e)}`;
    } finally {
      loading.value = false;
    }
  }

  function addKey(label: string, role: AccessRole): LanAccessKey {
    const key = createKey(label, role, 'key');
    keys.value.unshift(key);
    return key;
  }

  async function removeKey(id: string): Promise<boolean> {
    error.value = null;
    try {
      await api.deleteLanDeviceKey(id);
      keys.value = keys.value.filter((key) => key.id !== id);
      peerKeys.value = peerKeys.value.filter((key) => key.id !== id);
      return true;
    } catch (e) {
      error.value = '删除访问密钥失败：' + errorText(e);
      return false;
    }
  }

  /** 设备访问密钥走独立端点，和网站访问密钥/网站开关完全解耦。 */
  async function addDeviceKey(label: string, role: AccessRole): Promise<LanAccessKey | null> {
    error.value = null;
    try {
      const key = await api.createLanDeviceKey({ label, role });
      peerKeys.value.unshift(key);
      return key;
    } catch (e) {
      error.value = '创建设备密钥失败：' + errorText(e);
      return null;
    }
  }

  async function setDeviceKeyRole(id: string, role: AccessRole): Promise<boolean> {
    error.value = null;
    try {
      const updated = await api.patchLanDeviceKey(id, { role });
      const index = peerKeys.value.findIndex((key) => key.id === id);
      if (index >= 0) {
        peerKeys.value[index] = updated;
      }
      const webIndex = keys.value.findIndex((key) => key.id === id);
      if (webIndex >= 0) {
        keys.value[webIndex] = updated;
      }
      return true;
    } catch (e) {
      error.value = '更新设备密钥失败：' + errorText(e);
      return false;
    }
  }

  async function removeDeviceKey(id: string): Promise<boolean> {
    error.value = null;
    try {
      await api.deleteLanDeviceKey(id);
      // LAN Web / 原生 Peer / 远程设置共用同一 AccessKey 注册表，本地两个视图同步移除。
      keys.value = keys.value.filter((key) => key.id !== id);
      peerKeys.value = peerKeys.value.filter((key) => key.id !== id);
      return true;
    } catch (e) {
      error.value = '删除设备密钥失败：' + errorText(e);
      return false;
    }
  }

  function createKey(label: string, role: AccessRole, prefix: string): LanAccessKey {
    const bytes = new Uint8Array(32);
    crypto.getRandomValues(bytes);
    const access_key = [...bytes].map((b) => b.toString(16).padStart(2, '0')).join('');
    const idBytes = new Uint8Array(8);
    crypto.getRandomValues(idBytes);
    const id = `${prefix}_${[...idBytes].map((b) => b.toString(16).padStart(2, '0')).join('')}`;
    return {
      id,
      label: label.trim() || `访问密钥 ${(prefix === 'peer' ? peerKeys.value.length : keys.value.length) + 1}`,
      access_key,
      role,
      created_at_ms: Date.now(),
    };
  }

  async function save(): Promise<boolean> {
    if (saving.value) {
      return false;
    }
    if (!portValid.value || !peerPortValid.value) {
      error.value = PORT_ERROR;
      return false;
    }
    if (port.value.trim() === peerPort.value.trim()) {
      error.value = '网站端口与 hawk-app 直连端口不能相同';
      return false;
    }
    // First enable: auto-create a read-only key so the listener starts immediately.
    if (enabled.value && keys.value.length === 0) {
      addKey('\u7f51\u7ad9\u8bbf\u95ee\u5bc6\u94a5', 'viewer');
    }
    saving.value = true;
    error.value = null;
    try {
      apply(
        await api.saveAppLan({
          enabled: enabled.value,
          port: Number(port.value.trim()),
          peer_port: Number(peerPort.value.trim()),
          keys: keys.value,
          peer_keys: peerKeys.value,
        }),
      );
      loaded.value = true;
      return true;
    } catch (e) {
      error.value = `应用失败：${errorText(e)}`;
      return false;
    } finally {
      saving.value = false;
    }
  }

  return {
    loaded,
    loading,
    saving,
    error,
    enabled,
    port,
    peerPort,
    keys,
    peerKeys,
    active,
    peerActive,
    peerError,
    targetIdentity,
    tlsFingerprint,
    targetDeviceId,
    targetLibraryId,
    portValid,
    peerPortValid,
    PORT_ERROR,
    load,
    addKey,
    removeKey,
    addDeviceKey,
    setDeviceKeyRole,
    removeDeviceKey,
    save,
  };
});

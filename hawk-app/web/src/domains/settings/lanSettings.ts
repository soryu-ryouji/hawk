import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '@/shared/api/endpoints';
import { errorText } from '@/shared/lib/storeUtil';

export interface LanAccessKey {
  id: string;
  label: string;
  token: string;
  write: boolean;
  created_at_ms: number;
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
  }): void {
    enabled.value = dto.enabled;
    port.value = String(dto.port);
    peerPort.value = String(dto.peer_port);
    keys.value = dto.keys.map((key) => ({ ...key }));
    peerKeys.value = dto.peer_keys.map((key) => ({ ...key }));
    active.value = dto.active;
    peerActive.value = dto.peer_active;
    peerError.value = dto.peer_error ?? null;
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

  function addKey(label: string, write: boolean): LanAccessKey {
    const key = createKey(label, write, 'key');
    keys.value.unshift(key);
    return key;
  }

  function removeKey(id: string): void {
    keys.value = keys.value.filter((key) => key.id !== id);
  }

  function addPeerKey(label: string, write: boolean): LanAccessKey {
    const key = createKey(label, write, 'peer');
    peerKeys.value.unshift(key);
    return key;
  }

  function removePeerKey(id: string): void {
    peerKeys.value = peerKeys.value.filter((key) => key.id !== id);
  }

  function createKey(label: string, write: boolean, prefix: string): LanAccessKey {
    const bytes = new Uint8Array(32);
    crypto.getRandomValues(bytes);
    const token = [...bytes].map((b) => b.toString(16).padStart(2, '0')).join('');
    const idBytes = new Uint8Array(8);
    crypto.getRandomValues(idBytes);
    const id = `${prefix}_${[...idBytes].map((b) => b.toString(16).padStart(2, '0')).join('')}`;
    return {
      id,
      label: label.trim() || `访问密钥 ${(prefix === 'peer' ? peerKeys.value.length : keys.value.length) + 1}`,
      token,
      write,
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
      addKey('\u7f51\u7ad9\u8bbf\u95ee\u5bc6\u94a5', false);
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
    portValid,
    peerPortValid,
    PORT_ERROR,
    load,
    addKey,
    removeKey,
    addPeerKey,
    removePeerKey,
    save,
  };
});

<script setup lang="ts">
// 已授权设备：合并可反向连接的已配对设备与目标端设备密钥，统一编辑权限/删除/连接。
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { shell } from '@/shared/lib/platform';
import { errorText } from '@/shared/lib/storeUtil';
import { useLanStore } from '@/domains/lan';
import { useConnectionLabel } from '@/domains/remote';
import Icon from '@/shared/ui/Icon.vue';
import PairingQrDialog from './PairingQrDialog.vue';
import { encodePairingPayload, lanPairingPayload } from '../pairing';
import { useLanSettingsStore, type LanAccessKey } from '../lanSettings';
import type { LanClient, LanPeerProfile } from '@/shared/types';

interface DeviceRow {
  id: string;
  name: string;
  library_name: string;
  url: string;
  profile?: LanPeerProfile;
  accessKey?: LanAccessKey;
  online: boolean;
  current: boolean;
}

const lanStore = useLanStore();
const lan = useLanSettingsStore();
const connection = useConnectionLabel();
const devicesOpen = ref(false);
const busyId = ref<string | null>(null);
const error = ref('');
const notice = ref('');
const addresses = ref<string[]>([]);
const clients = ref<Record<string, LanClient>>({});
const editTarget = ref<DeviceRow | null>(null);
const editWrite = ref(false);
const qrPermissionOpen = ref(false);
const qrWrite = ref(false);
const generatingQr = ref(false);
const pairing = ref<{ payload: string; details: { label: string; value: string }[]; warning: string } | null>(null);
let clientTimer: ReturnType<typeof setInterval> | null = null;

const devices = computed<DeviceRow[]>(() => {
  const rows = new Map<string, DeviceRow>();
  for (const profile of lanStore.saved) {
    rows.set(profile.id, {
      id: profile.id,
      name: profile.name,
      library_name: profile.library_name,
      url: profile.url,
      profile,
      online: connection.isLan.value && connection.peerId.value === profile.id,
      current: connection.isLan.value && connection.peerId.value === profile.id,
    });
  }
  for (const key of lan.peerKeys) {
    const matched = key.peer_id ? rows.get(key.peer_id) : undefined;
    if (matched) {
      matched.accessKey = key;
      matched.library_name = matched.library_name || key.library_name || '';
      matched.url = matched.url || key.url || '';
      matched.online = matched.online || Boolean(clients.value[key.id]?.online);
    } else {
      rows.set(key.peer_id || key.id, {
        id: key.peer_id || key.id,
        name: key.label,
        library_name: key.library_name || '',
        url: key.url || '',
        accessKey: key,
        online: Boolean(clients.value[key.id]?.online),
        current: false,
      });
    }
  }
  return [...rows.values()].sort((a, b) => Number(b.current) - Number(a.current) || Number(b.online) - Number(a.online) || a.name.localeCompare(b.name));
});
const onlineCount = computed(() => devices.value.filter((device) => device.online || device.current).length);

async function refreshClients(): Promise<void> {
  try {
    const { clients: list } = await shell.lanClients();
    clients.value = Object.fromEntries(list.map((client) => [client.key_id, client]));
  } catch {
    // daemon 尚未就绪时静默
  }
}

async function refresh(): Promise<void> {
  await Promise.all([lanStore.refreshSaved(), lan.load(true)]);
  await refreshClients();
}

onMounted(async () => {
  await refresh();
  try {
    addresses.value = await shell.lanAddresses();
  } catch (e) {
    error.value = '读取本机地址失败：' + errorText(e);
  }
  clientTimer = setInterval(() => void refreshClients(), 4000);
});

onUnmounted(() => {
  if (clientTimer) {
    clearInterval(clientTimer);
  }
});

function toggleDevices(): void {
  devicesOpen.value = !devicesOpen.value;
  if (devicesOpen.value) {
    void refresh();
  }
}

function startEdit(device: DeviceRow): void {
  if (!device.accessKey) {
    error.value = '该设备没有本机设备密钥，无法修改本机授权权限';
    return;
  }
  error.value = '';
  editWrite.value = device.accessKey.role === 'editor';
  editTarget.value = device;
}

async function saveEdit(): Promise<void> {
  const device = editTarget.value;
  if (!device?.accessKey) {
    return;
  }
  busyId.value = device.accessKey.id;
  const ok = await lan.setDeviceKeyRole(device.accessKey.id, editWrite.value ? 'editor' : 'viewer');
  busyId.value = null;
  if (ok) {
    editTarget.value = null;
    notice.value = '设备权限已更新';
    window.setTimeout(() => (notice.value = ''), 3000);
  } else {
    error.value = lan.error ?? '更新设备权限失败';
  }
}

async function removeDevice(device: DeviceRow): Promise<void> {
  if (!window.confirm('停止与「' + device.name + '」的连接并撤销其访问权限？')) {
    return;
  }
  busyId.value = device.id;
  error.value = '';
  if (device.profile) {
    await lanStore.forget(device.profile.id);
  }
  if (device.accessKey) {
    await lan.removeDeviceKey(device.accessKey.id);
  }
  busyId.value = null;
  notice.value = '已移除设备';
  window.setTimeout(() => (notice.value = ''), 3000);
}

async function connectDevice(device: DeviceRow): Promise<void> {
  if (!device.profile || device.current) {
    return;
  }
  busyId.value = device.id;
  error.value = '';
  const ok = await lanStore.connect(device.profile.id);
  busyId.value = null;
  if (!ok) {
    error.value = lanStore.error ?? '连接失败';
  }
}

function requestQr(): void {
  if (generatingQr.value) {
    return;
  }
  error.value = '';
  qrWrite.value = false;
  qrPermissionOpen.value = true;
}

async function confirmQr(): Promise<void> {
  generatingQr.value = true;
  error.value = '';
  try {
    const key = await lan.addDeviceKey('移动端扫码 ' + (lan.peerKeys.length + 1), qrWrite.value ? 'editor' : 'viewer');
    if (!key) {
      error.value = lan.error ?? '创建设备密钥失败';
      return;
    }
    qrPermissionOpen.value = false;
    const urls = addresses.value.map((addr) => 'http://' + addr + ':' + lan.peerPort);
    pairing.value = {
      payload: encodePairingPayload(lanPairingPayload(urls, key.access_key, lan.targetIdentity, lan.targetLibraryId, lan.targetDeviceId, lan.tlsFingerprint)),
      details: [
        { label: '地址（客户端自动择优）', value: urls.join('\n') },
        { label: '访问密钥', value: key.access_key },
      ],
      warning: key.role === 'editor' ? '当前二维码包含写权限；请只在信任的设备上扫码。' : '',
    };
  } finally {
    generatingQr.value = false;
  }
}
</script>

<template>
  <section class="devices">
    <button class="devices-head" type="button" @click="toggleDevices">
      <span class="devices-title">已授权设备</span>
      <span class="devices-summary"
        >{{ devices.length }} 台<template v-if="onlineCount > 0"> · {{ onlineCount }} 在线</template></span
      >
      <Icon name="chevronDown" :size="14" :class="['devices-chevron', { open: devicesOpen }]" />
    </button>

    <div v-if="devicesOpen" class="devices-body">
      <div v-if="devices.length === 0" class="empty-state">暂无授权设备</div>
      <div v-for="device in devices" :key="device.id" class="device-row">
        <div class="device-main">
          <div class="device-name">
            {{ device.name }}
            <span v-if="device.library_name" class="lib">（{{ device.library_name }}）</span>
            <span v-if="device.current" class="badge current">已连接</span>
            <span v-else-if="device.online" class="badge online">在线</span>
            <span v-else class="badge offline">未连接</span>
          </div>
          <div class="device-meta">
            <span v-if="device.url">{{ device.url }}</span>
            <span v-if="device.accessKey"> · 本机权限：{{ device.accessKey.role === 'editor' ? '可读写' : '仅读取' }}</span>
          </div>
        </div>
        <div class="row-actions">
          <button v-if="device.profile && !device.current" :disabled="busyId !== null" @click="connectDevice(device)">
            {{ busyId === device.id ? '连接中…' : '连接' }}
          </button>
          <button class="icon-btn" title="编辑权限" :disabled="!device.accessKey || busyId !== null" @click="startEdit(device)">
            <Icon name="edit" :size="13" />
          </button>
          <button class="icon-btn" title="删除设备" :disabled="busyId !== null" @click="removeDevice(device)">
            <Icon name="trash" :size="13" />
          </button>
        </div>
      </div>

      <p v-if="notice" class="ok">{{ notice }}</p>
      <p v-if="error" class="error">{{ error }}</p>
      <button class="qr-button" :disabled="generatingQr" @click="requestQr">生成授权二维码</button>
    </div>

    <Teleport to="body">
      <div v-if="editTarget" class="modal-mask" @click.self="editTarget = null">
        <div class="modal">
          <div class="modal-title">编辑设备权限</div>
          <p class="modal-copy">{{ editTarget.name }}</p>
          <div v-if="editTarget.accessKey" class="perm-options">
            <label class="perm-option" :class="{ active: !editWrite }">
              <input v-model="editWrite" type="radio" :value="false" />
              <span><strong>仅读取</strong></span>
            </label>
            <label class="perm-option" :class="{ active: editWrite }">
              <input v-model="editWrite" type="radio" :value="true" />
              <span><strong>可读写</strong></span>
            </label>
          </div>
          <p v-if="editTarget.profile" class="modal-hint">对方授予本机：{{ editTarget.profile.writable ? '可读写' : '仅读取' }}</p>
          <div class="modal-actions">
            <button @click="editTarget = null">取消</button>
            <button class="primary" :disabled="!editTarget.accessKey || busyId !== null" @click="saveEdit">保存</button>
          </div>
        </div>
      </div>

      <div v-if="qrPermissionOpen" class="modal-mask" @click.self="qrPermissionOpen = false">
        <div class="modal">
          <div class="modal-title">生成移动端授权二维码</div>
          <div class="perm-options">
            <label class="perm-option" :class="{ active: !qrWrite }">
              <input v-model="qrWrite" type="radio" :value="false" />
              <span><strong>仅读取</strong></span>
            </label>
            <label class="perm-option" :class="{ active: qrWrite }">
              <input v-model="qrWrite" type="radio" :value="true" />
              <span><strong>可读写</strong></span>
            </label>
          </div>
          <div class="modal-actions">
            <button @click="qrPermissionOpen = false">取消</button>
            <button class="primary" :disabled="generatingQr" @click="confirmQr">{{ generatingQr ? '生成中…' : '生成二维码' }}</button>
          </div>
        </div>
      </div>
    </Teleport>

    <PairingQrDialog
      v-if="pairing"
      title="移动端局域网授权"
      :payload="pairing.payload"
      :details="pairing.details"
      note="用 hawk iOS / Android 客户端扫码连接此素材库。"
      :warning="pairing.warning"
      @close="pairing = null"
    />
  </section>
</template>
<style src="./settings-shared.css"></style>

<style scoped>
.devices {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.devices-head {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 4px 0;
  border: none;
  background: transparent;
  color: var(--fg-1);
  text-align: left;
}
.devices-title {
  font-size: 12px;
  font-weight: 600;
}
.devices-summary {
  color: var(--fg-1);
  font-size: 11px;
}
.devices-chevron {
  margin-left: auto;
  transition: transform 120ms ease;
}
.devices-chevron.open {
  transform: rotate(180deg);
}
.devices-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.device-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 7px 0;
  border-bottom: 1px solid var(--border);
}
.device-row:last-of-type {
  border-bottom: none;
}
.device-main {
  min-width: 0;
}
.device-name {
  overflow: hidden;
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.lib,
.device-meta {
  color: var(--fg-1);
}
.device-meta {
  margin-top: 2px;
  font-size: 11px;
  overflow-wrap: anywhere;
}
.badge {
  margin-left: 6px;
  padding: 0 5px;
  border: 1px solid var(--accent);
  border-radius: 3px;
  color: var(--accent);
  font-size: 10px;
  font-weight: 400;
}
.badge.offline {
  color: var(--fg-1);
  border-color: var(--border);
}
.row-actions {
  display: flex;
  align-items: center;
  gap: 5px;
  flex: none;
}
.row-actions button,
.qr-button {
  padding: 4px 9px;
  font-size: 12px;
}
.qr-button {
  align-self: flex-start;
  margin-top: 4px;
}
.empty-state {
  padding: 8px 0;
  color: var(--fg-1);
  font-size: 12px;
}
.ok {
  color: var(--accent);
  font-size: 12px;
}
.error {
  color: var(--danger);
  font-size: 12px;
}
.modal-mask {
  position: fixed;
  inset: 0;
  z-index: 230;
  display: grid;
  place-items: center;
  padding: 16px;
  background: rgba(0, 0, 0, 0.52);
}
.modal {
  width: min(380px, 100%);
  padding: 16px;
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--bg-1);
  box-shadow: 0 14px 40px rgba(0, 0, 0, 0.35);
}
.modal-title {
  margin-bottom: 6px;
  font-size: 14px;
  font-weight: 600;
}
.modal-copy {
  margin: 0 0 12px;
  color: var(--fg-0);
  font-size: 13px;
}
.modal-hint {
  margin: 10px 0 0;
  color: var(--fg-1);
  font-size: 11px;
}
.perm-options {
  display: flex;
  flex-direction: column;
  gap: 7px;
}
.perm-option {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  cursor: pointer;
}
.perm-option.active {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 8%, transparent);
}
.perm-option input {
  accent-color: var(--accent);
}
.modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}
.modal-actions button {
  padding: 5px 10px;
  font-size: 12px;
}
</style>

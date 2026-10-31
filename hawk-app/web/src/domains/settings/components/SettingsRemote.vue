<script setup lang="ts">
// 远程访问分区（仅 Electron）：接入自托管 hawk-server（应用级 remote.toml）+ 访问密钥管理 +
// 设备列表与会话入口。凭证模型：tenant-key（池门禁，server 侧 tenant add 生成）准入后，
//  hawk-app key（本机签发，逐把读/写权限）分发给网页版/插件等外部查看者。
// 配置读写/设备列表/密钥管理/会话动作全部经 IPC 委托主进程（切到远程后渲染进程的 API base 已指向
// 会话代理，管理面必须在主进程）。设置面板内保存由 footer 委托，RemoteScreen 保留内联保存；打开期间轮询信令状态。
// 错误条在主组件（v-model:error）；分区切换 v-show 保活（编辑中的字段不丢）。
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useRemoteStore, useConnectionLabel } from '@/domains/remote';
import { useLibraryStore } from '@/domains/library';
import PairingQrDialog from './PairingQrDialog.vue';
import Icon from '@/shared/ui/Icon.vue';
import { encodePairingPayload, remotePairingPayload } from '../pairing';
import type { RemoteKeyCreated } from '@/shared/types';

/** libraryless：远程查看模式（本地无库）——隐藏分享向开关（没有本地素材可服务）；
 *  设置对话框内（有本地库）不传 */
const props = defineProps<{ libraryless?: boolean; embedded?: boolean }>();
const error = defineModel<string | null>('error', { default: null });
const store = useRemoteStore();
const connection = useConnectionLabel();
/** 远端会话的写能力（只读 / 仅导入 / 可读写）来自目标库的 app/info */
const library = useLibraryStore();

const enabled = ref(false);
const url = ref('');
const tenantKey = ref('');
const writable = ref(false);
const saving = ref(false);

// ---- 本机设备管理（改名 / 远程开关 / 证书轮换） ----
const selfName = ref('');
const renaming = ref(false);
const rotating = ref(false);

// ---- 访问密钥管理 ----
const keyLabel = ref('');
const keyPermission = ref<'read' | 'write'>('read');
const keyCustom = ref('');
const creating = ref(false);
const keyCreateOpen = ref(false);
/** 刚签发的密钥（明文只在创建响应里，展示到用户关闭为止） */
const createdKey = ref<RemoteKeyCreated | null>(null);
const copied = ref('');
/** 配对二维码（由「生成配对二维码」签发的只读密钥生成） */
const pairing = ref<{ payload: string; details: { label: string; value: string }[]; warning: string } | null>(null);
const pairingBusy = ref(false);

onMounted(() => {
  store.startPolling();
  // 表单初值：读当前配置（store.dto 可能已被上次轮询填充）
  void store.refresh().then(() => {
    if (store.dto) {
      enabled.value = store.dto.enabled;
      url.value = store.dto.url;
      tenantKey.value = store.dto.tenant_key;
      writable.value = store.dto.allow_import;
    }
  });
  void store.refreshKeys();
});
onUnmounted(() => store.stopPolling());

const statusText = computed(() => {
  const s = store.dto?.status;
  if (!store.dto?.enabled) {
    return null;
  }
  switch (s?.state) {
    case 'connecting':
      // 断线重连时 error 携带原因（诊断用，设置面板直接可见）
      return s?.error ? `接入中…（${s.error}）` : '接入中…';
    case 'online':
      return `已在线（${s.server_url ?? ''}）`;
    case 'error':
      return s.error ?? '接入错误';
    default:
      return null;
  }
});

const statusLabel = computed(() => {
  if (!enabled.value) {
    return '未启用';
  }
  return statusText.value ?? (store.dto?.enabled ? '等待接入' : '未启用');
});

const statusTone = computed(() => {
  if (!enabled.value || !store.dto?.enabled) {
    return 'off';
  }
  switch (store.dto.status.state) {
    case 'online':
      return 'online';
    case 'connecting':
      return 'connecting';
    case 'error':
      return 'error';
    default:
      return 'off';
  }
});

/** 本机设备（设备列表中 device_id 与信令状态一致的一台） */
const selfDevice = computed(() => store.devices.find((d) => d.device_id === store.dto?.status.device_id) ?? null);

/** 其他设备：在线在前 */
const peerDevices = computed(() =>
  store.devices.filter((d) => d.device_id !== store.dto?.status.device_id).sort((a, b) => Number(b.online) - Number(a.online)),
);

async function save(): Promise<boolean> {
  error.value = null;
  saving.value = true;
  const ok = await store.saveConfig({
    enabled: enabled.value,
    url: url.value.trim(),
    tenant_key: tenantKey.value.trim(),
    allow_import: writable.value,
    relay: store.relayEnabled,
  });
  saving.value = false;
  if (!ok) {
    error.value = store.error;
  }
  return ok;
}

function busy(): boolean {
  return saving.value || pairingBusy.value || creating.value || rotating.value;
}

defineExpose({ save, busy });

/** 配对二维码可用条件：启用、服务端地址与服务密钥已填写（保存由按钮动作完成） */
const canPair = computed(() => enabled.value && url.value.trim().length > 0 && tenantKey.value.trim().length > 0);

/** 保存接入配置 → 签发只读访问密钥 → 生成移动端配对二维码 */
async function openPairing(): Promise<void> {
  if (!canPair.value || saving.value || pairingBusy.value) {
    return;
  }
  const ok = await save();
  if (!ok) {
    return;
  }
  const dto = store.dto;
  if (!dto?.enabled || !dto.url.trim() || !dto.tenant_key.trim()) {
    error.value = '请先保存并接入远程访问';
    return;
  }
  pairingBusy.value = true;
  const created = await store.createKey({ label: '移动端配对', role: 'viewer' });
  pairingBusy.value = false;
  if (!created) {
    error.value = store.error;
    return;
  }
  if (!created.target_device_id) {
    error.value = '目标设备尚未完成注册，无法生成广域网配对信息';
    return;
  }
  createdKey.value = created;
  const serverURL = dto.url.trim().replace(/\/+$/, '');
  const warnings: string[] = [];
  if (dto.status.state !== 'online') {
    warnings.push('远程信令当前未在线；目标设备上线后才能连接。');
  }
  pairing.value = {
    payload: encodePairingPayload(remotePairingPayload(serverURL, created.key, created.target_identity, created.target_device_id, created.target_library_id)),
    details: [
      { label: '服务器', value: serverURL },
      { label: '目标设备', value: created.target_device_id },
      { label: '访问密钥', value: created.key },
    ],
    warning: warnings.join(' '),
  };
}

// ---- 访问密钥动作 ----

function openKeyCreate(): void {
  error.value = null;
  createdKey.value = null;
  keyLabel.value = '';
  keyCustom.value = '';
  keyPermission.value = 'read';
  keyCreateOpen.value = true;
}

async function createKey() {
  error.value = null;
  createdKey.value = null;
  creating.value = true;
  const created = await store.createKey({
    label: keyLabel.value.trim() || undefined,
    role: keyPermission.value === 'write' ? 'importer' : 'viewer',
    custom: keyCustom.value.trim() || undefined,
  });
  creating.value = false;
  if (!created) {
    error.value = store.error;
    return;
  }
  createdKey.value = created;
  keyCreateOpen.value = false;
  keyLabel.value = '';
  keyCustom.value = '';
  keyPermission.value = 'read';
}

/** 复制文本（密钥/分享串）；button 文案短暂反馈 */
async function copyText(text: string, tag: string) {
  try {
    await navigator.clipboard.writeText(text);
    copied.value = tag;
    window.setTimeout(() => {
      if (copied.value === tag) {
        copied.value = '';
      }
    }, 1500);
  } catch {
    error.value = '复制失败';
  }
}

/** 分享串：查看者需要三段（服务器地址 + 服务密钥 + 访问密钥） */
function shareText(key: string): string {
  return [`服务器：${store.dto?.url ?? ''}`, `服务密钥：${store.dto?.tenant_key ?? ''}`, `访问密钥：${key}`].join('\n');
}

function keyPermissionValue(key: { role: string }): 'read' | 'write' {
  return key.role === 'importer' ? 'write' : 'read';
}

async function setKeyPermission(id: string, value: string) {
  error.value = null;
  const ok = await store.patchKey(id, { role: value === 'write' ? 'importer' : 'viewer' });
  if (!ok) {
    error.value = store.error;
  }
}

async function revokeKey(id: string, label: string) {
  if (!window.confirm(`吊销访问密钥「${label || id}」？持有该密钥的设备/插件将立即无法访问本库。`)) {
    return;
  }
  error.value = null;
  const ok = await store.deleteKey(id);
  if (!ok) {
    error.value = store.error;
  }
}

async function connect(deviceId: string) {
  error.value = null;
  await store.connect(deviceId);
  if (store.error) {
    error.value = store.error;
  }
}

async function disconnect() {
  await store.disconnect();
}

async function saveSelfName() {
  const name = selfName.value.trim();
  if (!name) {
    return;
  }
  renaming.value = true;
  await store.patchDevice({ name });
  renaming.value = false;
}

async function toggleAllowRemote(checked: boolean) {
  await store.patchDevice({ allow_remote: checked });
}

async function rotate() {
  error.value = null;
  rotating.value = true;
  const ok = await store.rotate();
  rotating.value = false;
  if (!ok) {
    error.value = store.error;
  }
}

/** 移除设备记录（本机当前设备不可移除，daemon 侧拦截）；列表轮询自动收敛 */
async function removeDevice(deviceId: string, name: string) {
  if (!window.confirm(`移除设备「${name}」的记录？其进行中的会话与访问密钥映射将一并失效。`)) {
    return;
  }
  error.value = null;
  const ok = await store.deleteDevice(deviceId);
  if (!ok) {
    error.value = store.error;
  }
}
</script>

<template>
  <div class="pane remote-pane" :class="{ embedded: props.embedded }">
    <div class="remote-head">
      <div class="remote-head-copy">
        <div class="remote-title">远程访问</div>
        <div class="remote-status">
          <span class="status-dot" :class="statusTone" />
          <span class="status-label">{{ statusLabel }}</span>
        </div>
      </div>
      <label class="switch" title="启用远程访问">
        <input v-model="enabled" type="checkbox" />
        <span class="track" />
      </label>
    </div>

    <div v-show="enabled" class="remote-content">
      <section class="settings-section">
        <div class="settings-section-title">接入</div>
        <div class="settings-group">
          <label class="remote-field">
            <span class="remote-field-label">服务器地址</span>
            <input v-model="url" class="settings-input" type="text" placeholder="https://hawk.example.com" autocomplete="off" spellcheck="false" />
          </label>
          <label class="remote-field">
            <span class="remote-field-label">服务密钥</span>
            <input
              v-model="tenantKey"
              class="settings-input"
              type="password"
              placeholder="hawk-server tenant add 生成的 hs_ 密钥"
              autocomplete="off"
              spellcheck="false"
            />
          </label>
          <div v-if="!props.libraryless" class="remote-switch-row">
            <span class="remote-field-label">允许广域网保存素材</span>
            <label class="switch" title="允许广域网会话导入素材">
              <input v-model="writable" type="checkbox" />
              <span class="track" />
            </label>
          </div>
        </div>
      </section>

      <section class="settings-section">
        <div class="settings-section-title">访问密钥</div>
        <div class="settings-group">
          <div class="remote-actions-row">
            <button v-if="!props.libraryless" class="remote-button" :disabled="!canPair || saving || pairingBusy" @click="openPairing">
              {{ pairingBusy ? '生成中…' : '生成配对二维码' }}
            </button>
            <button class="remote-button" @click="openKeyCreate">新建访问密钥</button>
          </div>

          <div v-if="createdKey" class="created-key">
            <div class="created-key-title">新访问密钥</div>
            <code class="created-key-code">{{ createdKey.key }}</code>
            <div class="created-key-actions">
              <button class="remote-button" @click="copyText(createdKey.key, 'key')">{{ copied === 'key' ? '已复制' : '复制密钥' }}</button>
              <button class="remote-button" @click="copyText(shareText(createdKey.key), 'share')">{{ copied === 'share' ? '已复制' : '复制分享串' }}</button>
              <button class="remote-button" @click="createdKey = null">完成</button>
            </div>
          </div>

          <div v-if="store.keys.length === 0" class="empty-row">暂无访问密钥</div>
          <div v-for="k in store.keys" :key="k.id" class="remote-row">
            <div class="row-main">
              <div class="row-title">{{ k.label || '未命名' }}</div>
              <div class="row-meta">{{ k.prefix }}…</div>
            </div>
            <select class="permission-select" :value="keyPermissionValue(k)" @change="setKeyPermission(k.id, ($event.target as HTMLSelectElement).value)">
              <option value="read">仅读取</option>
              <option value="write">可读写</option>
            </select>
            <button class="icon-btn" title="吊销访问密钥" @click="revokeKey(k.id, k.label)">
              <Icon name="trash" :size="13" />
            </button>
          </div>
        </div>
      </section>

      <template v-if="store.dto?.status.state === 'online'">
        <section v-if="selfDevice" class="settings-section">
          <div class="settings-section-title">此设备</div>
          <div class="settings-group">
            <div class="remote-field remote-field-action">
              <span class="remote-field-label">显示名称</span>
              <input v-model="selfName" class="settings-input" type="text" :placeholder="selfDevice.name" autocomplete="off" />
              <button class="remote-button" :disabled="renaming || !selfName.trim()" @click="saveSelfName">保存</button>
            </div>
            <div v-if="!props.libraryless" class="remote-switch-row">
              <span class="remote-field-label">允许其他设备访问本库</span>
              <label class="switch" title="允许其他设备访问本库">
                <input :checked="selfDevice.allow_remote" type="checkbox" @change="toggleAllowRemote(($event.target as HTMLInputElement).checked)" />
                <span class="track" />
              </label>
            </div>
            <div class="remote-row">
              <div class="row-main">
                <div class="row-title">设备证书</div>
                <div class="row-meta">用于远程信令与设备认证</div>
              </div>
              <button class="remote-button" :disabled="rotating" @click="rotate">{{ rotating ? '轮换中…' : '轮换' }}</button>
            </div>
          </div>
        </section>

        <section class="settings-section">
          <div class="settings-section-title">设备</div>
          <div class="settings-group">
            <div v-if="connection.isRemote.value" class="remote-session">
              <div class="row-main">
                <div class="row-title">已连接 {{ connection.label.value }}</div>
                <div class="row-meta">
                  {{ library.viewerMode ? '只读' : library.limitedWrite ? '仅导入' : '可读写' }}
                </div>
              </div>
              <button class="remote-button" @click="disconnect">断开</button>
            </div>
            <div class="remote-switch-row">
              <span class="remote-field-label">使用中继</span>
              <label class="switch" title="直连失败时经服务端使用中继">
                <input :checked="store.relayEnabled" type="checkbox" @change="store.setRelayEnabled(($event.target as HTMLInputElement).checked)" />
                <span class="track" />
              </label>
            </div>
            <div v-if="peerDevices.length === 0" class="empty-row">暂无其他设备</div>
            <div v-for="device in peerDevices" :key="device.device_id" class="remote-row">
              <span class="device-dot" :class="{ online: device.online }" />
              <div class="row-main">
                <div class="row-title">{{ device.name }}</div>
                <div class="row-meta">
                  {{ device.library_name }} · {{ device.platform }}{{ device.allow_remote ? '' : ' · 已关闭访问' }}{{ device.allow_import ? ' · 允许写入' : ''
                  }}{{ device.online ? (device.reachable === true ? ' · 直连' : device.reachable === false ? ' · 中继' : '') : '' }}
                </div>
              </div>
              <button v-if="device.online && device.allow_remote" class="primary" :disabled="store.connectingId !== null" @click="connect(device.device_id)">
                {{ store.connectingId === device.device_id ? '连接中…' : '浏览' }}
              </button>
              <span v-else class="row-status">{{ device.online ? '不可访问' : '离线' }}</span>
              <button class="icon-btn" title="移除设备记录" @click="removeDevice(device.device_id, device.name)">
                <Icon name="trash" :size="13" />
              </button>
            </div>
          </div>
        </section>
      </template>

      <div v-if="props.libraryless" class="inline-save">
        <button class="primary" :disabled="saving" @click="save">{{ saving ? '保存中…' : '保存并接入' }}</button>
      </div>
    </div>

    <Teleport to="body">
      <div v-if="keyCreateOpen" class="modal-mask" @click.self="keyCreateOpen = false">
        <div class="modal">
          <div class="modal-title">新建访问密钥</div>
          <label class="modal-field">
            <span>名称</span>
            <input v-model="keyLabel" class="settings-input" type="text" placeholder="例如：手机 / 平板" autocomplete="off" />
          </label>
          <label class="modal-field">
            <span>自定义密钥</span>
            <input v-model="keyCustom" class="settings-input" type="text" placeholder="留空自动生成" autocomplete="off" spellcheck="false" />
          </label>
          <div class="permission-toggle">
            <button :class="{ active: keyPermission === 'read' }" @click="keyPermission = 'read'">仅读取</button>
            <button :class="{ active: keyPermission === 'write' }" @click="keyPermission = 'write'">可读写</button>
          </div>
          <div class="modal-actions">
            <button @click="keyCreateOpen = false">取消</button>
            <button class="primary" :disabled="creating" @click="createKey">{{ creating ? '签发中…' : '签发' }}</button>
          </div>
        </div>
      </div>
    </Teleport>

    <PairingQrDialog
      v-if="pairing"
      title="广域网配对"
      :payload="pairing.payload"
      :details="pairing.details"
      note="用 hawk iOS / Android 客户端扫码，经自托管 hawk-server 远程浏览本素材库。"
      :warning="pairing.warning"
      @close="pairing = null"
    />
  </div>
</template>
<style src="./settings-shared.css"></style>

<style scoped>
.remote-pane {
  gap: 16px;
}

.remote-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 14px;
  padding: 1px 2px 3px;
}

.remote-head-copy {
  min-width: 0;
}

.remote-title {
  color: var(--fg-0);
  font-size: 15px;
  font-weight: 600;
  letter-spacing: -0.01em;
}

.remote-status {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  margin-top: 4px;
  color: var(--fg-1);
  font-size: 11px;
}

.status-label {
  overflow: hidden;
  max-width: 360px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.status-dot,
.device-dot {
  width: 7px;
  height: 7px;
  flex: none;
  border-radius: 50%;
  background: var(--fg-1);
  opacity: 0.35;
}

.status-dot.online,
.device-dot.online {
  background: var(--accent);
  opacity: 1;
}

.status-dot.connecting {
  background: #f5a623;
  opacity: 1;
}

.status-dot.error {
  background: var(--danger);
  opacity: 1;
}

.remote-content {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.settings-group > * + * {
  border-top: 1px solid var(--border);
}

.remote-field {
  display: grid;
  grid-template-columns: 96px minmax(0, 1fr);
  align-items: center;
  gap: 12px;
  padding: 8px 12px;
}

.remote-field-label {
  color: var(--fg-1);
  font-size: 12px;
}

.remote-field .settings-input {
  justify-self: stretch;
}

.remote-field-action {
  grid-template-columns: 96px minmax(0, 1fr) auto;
}

.remote-switch-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  min-height: 44px;
  padding: 8px 12px;
}

.remote-actions-row {
  display: flex;
  flex-wrap: wrap;
  gap: 7px;
  padding: 9px 12px;
}

.remote-button {
  height: 28px;
  padding: 0 11px;
  border: 1px solid var(--border);
  border-radius: 7px;
  background: var(--bg-1);
  color: var(--fg-0);
  font-size: 12px;
}

.remote-button:hover:not(:disabled) {
  border-color: var(--fg-1);
}

.created-key {
  display: flex;
  flex-direction: column;
  gap: 7px;
  padding: 10px 12px;
  background: color-mix(in srgb, var(--accent) 7%, transparent);
}

.created-key-title {
  color: var(--fg-0);
  font-size: 12px;
  font-weight: 600;
}

.created-key-code {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 11px;
  overflow-wrap: anywhere;
  user-select: all;
}

.created-key-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.empty-row {
  padding: 11px 12px;
  color: var(--fg-1);
  font-size: 12px;
}

.remote-row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 44px;
  padding: 8px 12px;
}

.row-main {
  flex: 1;
  min-width: 0;
}

.row-title {
  overflow: hidden;
  color: var(--fg-0);
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row-meta {
  margin-top: 2px;
  color: var(--fg-1);
  font-size: 11px;
  overflow-wrap: anywhere;
}

.row-status {
  flex: none;
  color: var(--fg-1);
  font-size: 11px;
}

.permission-select {
  height: 28px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: 7px;
  background: var(--bg-1);
  color: var(--fg-0);
  font-size: 11px;
}

.remote-session {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 12px;
  background: color-mix(in srgb, var(--accent) 6%, transparent);
}

.inline-save {
  display: flex;
  justify-content: flex-end;
  padding-top: 2px;
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
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: min(380px, 100%);
  padding: 16px;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--bg-1);
  box-shadow: 0 14px 40px rgba(0, 0, 0, 0.35);
}

.modal-title {
  color: var(--fg-0);
  font-size: 14px;
  font-weight: 600;
}

.modal-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
  color: var(--fg-1);
  font-size: 11px;
}

.permission-toggle {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 3px;
  padding: 3px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-2);
}

.permission-toggle button {
  height: 27px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--fg-1);
  font-size: 12px;
}

.permission-toggle button.active {
  background: var(--bg-1);
  color: var(--fg-0);
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.16);
}

.modal-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 2px;
}

.modal-actions button {
  min-width: 64px;
  height: 30px;
  padding: 0 12px;
  border: 1px solid var(--border);
  border-radius: 7px;
  background: var(--bg-2);
  color: var(--fg-0);
  font-size: 12px;
}

@media (max-width: 520px) {
  .remote-field {
    grid-template-columns: 1fr;
    gap: 6px;
  }

  .remote-field-action {
    grid-template-columns: minmax(0, 1fr) auto;
  }

  .remote-field-action .remote-field-label {
    grid-column: 1 / -1;
  }
}
</style>

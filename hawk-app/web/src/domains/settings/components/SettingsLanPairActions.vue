<script setup lang="ts">
// 目标端共享管理：已授权设备权限/撤销，以及移动端扫码授权。
// 桌面 hawk-app 互连统一走配对请求；这里不再生成配对码或暴露密钥选择。
import { onMounted, ref } from 'vue';
import { errorText } from '@/shared/lib/storeUtil';
import { shell } from '@/shared/lib/platform';
import Icon from '@/shared/ui/Icon.vue';
import PairingQrDialog from './PairingQrDialog.vue';
import { encodePairingPayload, lanPairingPayload } from '../pairing';
import { useLanSettingsStore, type LanAccessKey } from '../lanSettings';

const lan = useLanSettingsStore();
const addresses = ref<string[]>([]);
const busyKeyId = ref<string | null>(null);
const error = ref('');
const showQrPermission = ref(false);
const newKeyWrite = ref(false);
const generating = ref(false);
const pairing = ref<{ payload: string; details: { label: string; value: string }[]; warning: string } | null>(null);

onMounted(async () => {
  await lan.load();
  if (lan.error) {
    error.value = lan.error;
  }
  try {
    addresses.value = await shell.lanAddresses();
  } catch (e) {
    error.value = `读取本机地址失败：${errorText(e)}`;
  }
});

/** 目标端动态调整某台已授权设备的读写权限；鉴权中间件每请求读取，保存后立即生效。 */
async function setPeerWrite(id: string, write: boolean): Promise<void> {
  if (busyKeyId.value) {
    return;
  }
  const key = lan.peerKeys.find((item) => item.id === id);
  if (!key) {
    return;
  }
  busyKeyId.value = id;
  key.write = write;
  if (!(await lan.save())) {
    error.value = lan.error ?? '权限更新失败';
  }
  busyKeyId.value = null;
}

/** 撤销某台设备的共享权限：删除其专用直连密钥，对方后续请求会 401。 */
async function revokePeer(id: string, label: string): Promise<void> {
  if (busyKeyId.value || !window.confirm(`停止与「${label}」共享素材库？`)) {
    return;
  }
  busyKeyId.value = id;
  lan.removePeerKey(id);
  if (!(await lan.save())) {
    error.value = lan.error ?? '撤销共享失败';
  }
  busyKeyId.value = null;
}

/** 打开二维码权限选择；确认后自动创建一把移动端专用密钥并展示二维码。 */
function requestQr(): void {
  if (lan.loading || generating.value) {
    return;
  }
  error.value = '';
  newKeyWrite.value = false;
  showQrPermission.value = true;
}

async function confirmQr(): Promise<void> {
  if (generating.value) {
    return;
  }
  generating.value = true;
  error.value = '';
  const key = lan.addPeerKey(`移动端扫码 ${lan.peerKeys.length + 1}`, newKeyWrite.value);
  try {
    if (!(await lan.save())) {
      lan.removePeerKey(key.id);
      error.value = lan.error ?? '保存局域网配置失败';
      return;
    }
    const saved = lan.peerKeys.find((item) => item.id === key.id) ?? key;
    showQrPermission.value = false;
    openQr(saved);
  } finally {
    generating.value = false;
  }
}

function openQr(key: LanAccessKey): void {
  const urls = addresses.value.map((addr) => `http://${addr}:${lan.peerPort}`);
  const warnings: string[] = [];
  if (key.write) {
    warnings.push('当前二维码包含写权限；请只在信任的设备上扫码。');
  }
  pairing.value = {
    payload: encodePairingPayload(lanPairingPayload(urls, key.token)),
    details: [
      { label: '地址（客户端自动择优）', value: urls.join('\n') },
      { label: '访问密钥', value: key.token },
    ],
    warning: warnings.join(' '),
  };
}
</script>

<template>
  <div class="pair-actions">
    <div class="group">
      <div class="group-title">已授权设备</div>
      <div v-if="lan.peerKeys.length === 0" class="empty-state">暂无授权设备</div>
      <div v-for="key in lan.peerKeys" :key="key.id" class="peer-row">
        <div class="peer-main">
          <div class="peer-name">{{ key.label }}</div>
          <div class="peer-meta">{{ key.write ? '可读写' : '只读' }}</div>
        </div>
        <div class="row-actions">
          <button :disabled="busyKeyId !== null" @click="setPeerWrite(key.id, !key.write)">
            {{ key.write ? '设为只读' : '允许写入' }}
          </button>
          <button class="icon-btn" title="停止共享" :disabled="busyKeyId !== null" @click="revokePeer(key.id, key.label)">
            <Icon name="trash" :size="13" />
          </button>
        </div>
      </div>
    </div>

    <div class="button-row">
      <button :disabled="lan.loading || generating" @click="requestQr">生成授权二维码</button>
    </div>
    <p v-if="!lan.peerActive" class="hint warning">{{ lan.peerError || 'hawk-app 直连监听未生效' }}</p>
    <p v-if="error" class="error">{{ error }}</p>

    <Teleport to="body">
      <div v-if="showQrPermission" class="key-dialog-backdrop" @click.self="showQrPermission = false">
        <div class="key-dialog" role="dialog" aria-modal="true" aria-labelledby="key-dialog-title">
          <div id="key-dialog-title" class="key-dialog-title">生成移动端授权二维码</div>
          <div class="perm-options">
            <label class="perm-option" :class="{ active: !newKeyWrite }">
              <input v-model="newKeyWrite" type="radio" :value="false" />
              <span><strong>仅读取</strong></span>
            </label>
            <label class="perm-option" :class="{ active: newKeyWrite }">
              <input v-model="newKeyWrite" type="radio" :value="true" />
              <span><strong>可读写</strong></span>
            </label>
          </div>
          <div class="key-dialog-actions">
            <button :disabled="generating" @click="showQrPermission = false">取消</button>
            <button class="primary" :disabled="generating" @click="confirmQr">
              {{ generating ? '生成中…' : '生成二维码' }}
            </button>
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
  </div>
</template>

<style src="./settings-shared.css"></style>

<style scoped>
.pair-actions {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.group {
  display: flex;
  flex-direction: column;
}
.group-title {
  padding: 2px 0 5px;
  color: var(--fg-1);
  font-size: 11px;
  font-weight: 600;
}
.peer-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 7px 0;
  border-bottom: 1px solid var(--border);
}
.peer-row:last-child {
  border-bottom: none;
}
.peer-main {
  min-width: 0;
}
.peer-name {
  overflow: hidden;
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.peer-meta {
  margin-top: 2px;
  color: var(--fg-1);
  font-size: 11px;
}
.row-actions,
.button-row {
  display: flex;
  align-items: center;
  gap: 5px;
  flex-wrap: wrap;
}
.row-actions {
  flex: none;
}
.row-actions button,
.button-row button {
  font-size: 12px;
  padding: 4px 9px;
}
.empty-state {
  padding: 8px 0;
  color: var(--fg-1);
  font-size: 12px;
}
.hint {
  margin: 0;
  color: var(--fg-1);
  font-size: 12px;
}
.warning,
.error {
  color: var(--danger);
  font-size: 12px;
}
.key-dialog-backdrop {
  position: fixed;
  inset: 0;
  z-index: 220;
  display: grid;
  place-items: center;
  padding: 16px;
  background: rgba(0, 0, 0, 0.5);
}
.key-dialog {
  width: min(360px, 100%);
  padding: 16px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--bg-1);
  box-shadow: 0 14px 40px rgba(0, 0, 0, 0.35);
}
.key-dialog-title {
  margin-bottom: 6px;
  font-size: 14px;
  font-weight: 600;
}
.perm-options {
  display: flex;
  flex-direction: column;
  gap: 7px;
  margin-top: 12px;
}
.perm-option {
  display: flex;
  align-items: flex-start;
  gap: 9px;
  padding: 9px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  cursor: pointer;
}
.perm-option.active {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 8%, transparent);
}
.perm-option input {
  margin-top: 2px;
  accent-color: var(--accent);
}
.perm-option strong {
  font-size: 13px;
  font-weight: 500;
}
.key-dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}
.key-dialog-actions button {
  padding: 5px 10px;
  font-size: 12px;
}
</style>

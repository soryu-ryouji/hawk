<script setup lang="ts">
// 访问其他设备 → 配对操作：生成一次性配对码 / iOS 扫码二维码。
// 与「局域网网站访问」共享 useLanSettingsStore；没有直连密钥时点击生成会弹窗选择权限，
// 确认后自动创建并保存，不把密钥权限作为页面一级信息展示。
import { computed, onMounted, ref, watch } from 'vue';
import { useClipboard } from '@vueuse/core';
import { api } from '@/shared/api/endpoints';
import { errorText } from '@/shared/lib/storeUtil';
import { shell } from '@/shared/lib/platform';
import Icon from '@/shared/ui/Icon.vue';
import PairingCodeInput from './PairingCodeInput.vue';
import PairingQrDialog from './PairingQrDialog.vue';
import { encodePairingPayload, lanPairingPayload } from '../pairing';
import { useLanSettingsStore, type LanAccessKey } from '../lanSettings';

type PairMode = 'code' | 'qr';

const lan = useLanSettingsStore();
const addresses = ref<string[]>([]);
const selectedKeyId = ref('');
const generating = ref(false);
const error = ref('');
const code = ref('');
const pairing = ref<{ payload: string; details: { label: string; value: string }[]; warning: string } | null>(null);
const pendingMode = ref<PairMode | null>(null);
const newKeyWrite = ref(false);
const { copy } = useClipboard({ legacy: true });

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
  selectDefaultKey();
});

const selectedKey = computed(() => lan.peerKeys.find((key) => key.id === selectedKeyId.value) ?? lan.peerKeys[0] ?? null);
const canGenerate = computed(() => !lan.loading);

watch(
  () => lan.peerKeys.map((key) => key.id).join(','),
  () => selectDefaultKey(),
);

function selectDefaultKey(): void {
  if (lan.peerKeys.some((key) => key.id === selectedKeyId.value)) {
    return;
  }
  selectedKeyId.value = (lan.peerKeys.find((key) => !key.write) ?? lan.peerKeys[0])?.id ?? '';
}

function requestGenerate(mode: PairMode): void {
  if (!canGenerate.value || generating.value) {
    return;
  }
  if (mode === 'qr' && addresses.value.length === 0) {
    error.value = '未检测到局域网 IPv4 地址';
    return;
  }
  error.value = '';
  if (!selectedKey.value) {
    newKeyWrite.value = false;
    pendingMode.value = mode;
    return;
  }
  void generate(mode);
}

async function confirmCreateKey(): Promise<void> {
  const mode = pendingMode.value;
  if (!mode || generating.value) {
    return;
  }
  const key = lan.addPeerKey('配对密钥', newKeyWrite.value);
  pendingMode.value = null;
  await generate(mode, key);
}

async function generate(mode: PairMode, created?: LanAccessKey): Promise<void> {
  if (generating.value) {
    return;
  }
  generating.value = true;
  error.value = '';
  try {
    let key = created ?? selectedKey.value;
    if (!key) {
      return;
    }
    // 新 key 可能只存在前端草稿中；生成前先保存，保证 key 已落盘且监听就绪。
    if (!(await lan.save())) {
      error.value = lan.error ?? '保存局域网配置失败';
      return;
    }
    selectedKeyId.value = key.id;
    key = lan.peerKeys.find((item) => item.id === key!.id) ?? key;
    if (mode === 'code') {
      code.value = (await api.createLanPairCode({ key_id: key.id })).code;
    } else {
      openQr(key);
    }
  } catch (e) {
    error.value = `${mode === 'code' ? '生成配对码' : '生成配对二维码'}失败：${errorText(e)}`;
  } finally {
    generating.value = false;
  }
}

function openQr(key: LanAccessKey): void {
  const urls = addresses.value.map((addr) => `http://${addr}:${lan.peerPort}`);
  const warnings: string[] = [];
  if (key.write) {
    warnings.push('当前选择的密钥具备写权限；请只在信任的设备上扫码。');
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

function copyCode(): void {
  if (code.value) {
    void copy(code.value);
  }
}
</script>

<template>
  <div class="pair-actions">
    <div v-if="lan.peerKeys.length > 0" class="pair-controls">
      <label class="control">
        <span>使用密钥</span>
        <select v-model="selectedKeyId">
          <option v-for="key in lan.peerKeys" :key="key.id" :value="key.id">{{ key.label }}（{{ key.write ? '读写' : '只读' }}）</option>
        </select>
      </label>
    </div>

    <div class="button-row">
      <button :disabled="!canGenerate || generating" @click="requestGenerate('code')">
        {{ generating ? '生成中…' : '生成配对码' }}
      </button>
      <button :disabled="!canGenerate || generating" @click="requestGenerate('qr')">生成配对二维码</button>
    </div>

    <p v-if="lan.peerKeys.length > 0 && !lan.peerActive" class="hint warning">{{ lan.peerError || 'hawk-app 直连监听未生效' }}</p>

    <div v-if="code" class="code-row">
      <PairingCodeInput :model-value="code" readonly label="hawk-app 配对码" />
      <button class="copy-btn" @click="copyCode">
        <Icon name="copy" :size="12" />
        复制
      </button>
    </div>
    <p v-if="error" class="error">{{ error }}</p>

    <Teleport to="body">
      <div v-if="pendingMode" class="key-dialog-backdrop" @click.self="pendingMode = null">
        <div class="key-dialog" role="dialog" aria-modal="true" aria-labelledby="key-dialog-title">
          <div id="key-dialog-title" class="key-dialog-title">创建 hawk-app 直连密钥</div>
          <div class="perm-options">
            <label class="perm-option" :class="{ active: !newKeyWrite }">
              <input v-model="newKeyWrite" type="radio" :value="false" />
              <span><strong>只读</strong></span>
            </label>
            <label class="perm-option" :class="{ active: newKeyWrite }">
              <input v-model="newKeyWrite" type="radio" :value="true" />
              <span><strong>允许写入</strong></span>
            </label>
          </div>
          <div class="key-dialog-actions">
            <button :disabled="generating" @click="pendingMode = null">取消</button>
            <button class="primary" :disabled="generating" @click="confirmCreateKey">
              {{ generating ? '创建中…' : '创建并使用' }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>

    <PairingQrDialog
      v-if="pairing"
      title="局域网配对"
      :payload="pairing.payload"
      :details="pairing.details"
      note="用 hawk iOS 客户端扫码连接同一局域网内的素材库。"
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
  gap: 8px;
}
.hint {
  margin: 0;
  color: var(--fg-1);
  font-size: 12px;
  line-height: 1.5;
}
.warning {
  color: var(--danger);
}
.pair-controls {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.control {
  display: flex;
  align-items: center;
  gap: 5px;
  color: var(--fg-1);
  font-size: 12px;
}
.control select {
  max-width: 175px;
}
.button-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.button-row > button {
  font-size: 12px;
  padding: 5px 10px;
}
.code-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-top: 2px;
}
.copy-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex: none;
  padding: 5px 9px;
  font-size: 12px;
}
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
.perm-option span {
  display: flex;
  flex-direction: column;
  gap: 2px;
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

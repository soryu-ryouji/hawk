<script setup lang="ts">
// 局域网网站访问：开关、访问地址与多把访问密钥。配置状态与「访问其他设备」「高级」共享
// useLanSettingsStore，创建/删除 key 后配对入口立即同步，不再各自加载出现陈旧状态。
import { computed, onMounted, ref } from 'vue';
import { useClipboard } from '@vueuse/core';
import { errorText } from '@/shared/lib/storeUtil';
import { useLibraryStore } from '@/domains/library';
import { shell } from '@/shared/lib/platform';
import Icon from '@/shared/ui/Icon.vue';
import { useLanSettingsStore } from '../lanSettings';

const store = useLibraryStore();
const lan = useLanSettingsStore();
const error = defineModel<string | null>('error', { default: null });
const props = defineProps<{ embedded?: boolean }>();

const addresses = ref<string[]>([]);
const keyLabel = ref('');
const keyWrite = ref(false);
const notice = ref('');
const { copy: copyText } = useClipboard({ legacy: true });

onMounted(async () => {
  await lan.load();
  error.value = lan.error;
  try {
    addresses.value = await shell.lanAddresses();
  } catch (e) {
    error.value = `读取本机地址失败：${errorText(e)}`;
  }
});

const portValid = computed(() => lan.portValid);

function addKey(): void {
  lan.addKey(keyLabel.value, keyWrite.value);
  keyLabel.value = '';
  keyWrite.value = false;
  error.value = null;
}

function removeKey(id: string): void {
  lan.removeKey(id);
}

function copy(value: string): void {
  void copyText(value);
  store.showToast('已复制到剪贴板');
}

function maskedToken(token: string): string {
  return `${token.slice(0, 6)}••••${token.slice(-4)}`;
}

async function save(): Promise<boolean> {
  notice.value = '';
  const ok = await lan.save();
  error.value = lan.error;
  if (ok) {
    notice.value = '网站设置已保存';
    window.setTimeout(() => {
      if (notice.value === '网站设置已保存') {
        notice.value = '';
      }
    }, 3000);
  }
  return ok;
}

function busy(): boolean {
  return lan.loading || lan.saving;
}

defineExpose({ save, busy });
</script>

<template>
  <div class="pane" :class="{ embedded: props.embedded }">
    <div class="switch-row">
      <div class="switch-label">启用局域网网站访问</div>
      <label class="switch" title="启用局域网网站访问">
        <input v-model="lan.enabled" type="checkbox" />
        <span class="track" />
      </label>
    </div>

    <p v-if="notice" class="settings-message ok">{{ notice }}</p>
    <div v-if="lan.loading" class="hint">读取设置中…</div>
    <template v-else>
      <div v-show="lan.enabled" class="lan-detail">
        <div class="field column">
          <span class="field-label">访问地址列表</span>
          <ul class="addrs">
            <li v-for="ip in addresses" :key="ip">
              <a :href="portValid ? `http://${ip}:${lan.port}` : undefined" target="_blank" rel="noreferrer">http://{{ ip }}:{{ lan.port }}</a>
              <button class="icon-btn" :disabled="!portValid" title="复制地址" @click="copy(`http://${ip}:${lan.port}`)">
                <Icon name="copy" :size="13" />
              </button>
            </li>
            <li v-if="addresses.length === 0" class="hint">未检测到局域网 IPv4 地址（检查本机网络连接）</li>
          </ul>
        </div>

        <div class="field column key-section">
          <span class="field-label">创建访问密钥 KEY</span>
          <div class="key-create">
            <input v-model="keyLabel" type="text" autocomplete="off" spellcheck="false" placeholder="备注，例如：我的手机" />
            <label class="key-write">
              <input v-model="keyWrite" type="checkbox" />
              允许写入
            </label>
            <button :disabled="lan.loading" @click="addKey">创建</button>
          </div>
        </div>

        <div class="field column">
          <span class="field-label">密钥列表</span>
          <div v-if="lan.keys.length > 0" class="key-list">
            <div v-for="key in lan.keys" :key="key.id" class="key-row">
              <div class="key-main">
                <div class="key-label">{{ key.label }}</div>
                <div class="key-token">{{ maskedToken(key.token) }}</div>
              </div>
              <span class="perm read">读</span>
              <span v-if="key.write" class="perm write">写</span>
              <div class="key-actions">
                <button class="icon-btn" title="复制完整密钥" @click="copy(key.token)">
                  <Icon name="copy" :size="13" />
                </button>
                <button class="icon-btn" title="删除访问密钥" @click="removeKey(key.id)">
                  <Icon name="trash" :size="13" />
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>

<style src="./settings-shared.css"></style>

<style scoped>
.lan-detail {
  display: flex;
  flex-direction: column;
  gap: 14px;
  margin-top: 4px;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}

.addrs {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.addrs li {
  display: flex;
  align-items: center;
  gap: 4px;
}

.addrs a {
  color: var(--accent);
  text-decoration: none;
  overflow-wrap: anywhere;
}

.key-section {
  gap: 8px;
}

.key-create {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.key-create > input[type='text'] {
  flex: 1;
  min-width: 130px;
  padding: 5px 8px;
}

.key-write {
  display: flex;
  align-items: center;
  gap: 5px;
  color: var(--fg-1);
  font-size: 12px;
  white-space: nowrap;
}

.key-write input {
  accent-color: var(--accent);
}

.key-list {
  display: flex;
  flex-direction: column;
}

.key-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 0;
  border-bottom: 1px solid var(--border);
}

.key-row:last-child {
  border-bottom: none;
}

.key-main {
  flex: 1;
  min-width: 0;
}

.key-label {
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.key-token {
  margin-top: 1px;
  color: var(--fg-1);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 11px;
}

.perm {
  flex: none;
  padding: 0 5px;
  border: 1px solid var(--border);
  border-radius: 3px;
  font-size: 10px;
  line-height: 1.7;
}

.perm.read {
  color: var(--fg-1);
}

.perm.write {
  color: var(--accent);
  border-color: var(--accent);
}

.key-actions {
  display: flex;
  gap: 2px;
  flex: none;
}
</style>

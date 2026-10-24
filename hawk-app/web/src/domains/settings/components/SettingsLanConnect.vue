<script setup lang="ts">
// 局域网互联（仅 Electron）：发现同网段其他 hawk-app，凭目标端一次性配对码建立直连。
// 目标端只需有 [web].peer_keys 直连密钥并生成配对码，不依赖局域网网站开关；源端不经过 hawk-server。
// token 由主进程保存，本组件只见设备展示信息。
import { computed, onMounted, ref, watch } from 'vue';
import { useLanStore } from '@/domains/lan';
import { ipcMessage, useConnectionLabel } from '@/domains/remote';
import { shell } from '@/shared/lib/platform';
import Icon from '@/shared/ui/Icon.vue';
import PairingCodeInput from './PairingCodeInput.vue';
import type { LanPeer, LanPeerProfile } from '@/shared/types';

const store = useLanStore();
const connection = useConnectionLabel();
const code = ref('');
const pairTarget = ref<LanPeer | null>(null);
const pairWritable = ref(false);
const savedMessage = ref<string | null>(null);
/** 手动配对兜底：广播/组播被访客网络隔离时，输入目标设置面板显示的访问地址 + 配对码 */
const manualOpen = ref(false);
const manualUrl = ref('');
const manualCode = ref('');
const manualWritable = ref(false);
/** 局域网测速（当前实现为 app/info 往返延迟中位数） */
const testingSpeed = ref(false);
const speedMs = ref<number | null>(null);
const isLanConnected = computed(() => connection.isLan.value);
const statusText = computed(() => {
  if (!isLanConnected.value) {
    return '当前未连接局域网设备';
  }
  return speedMs.value === null ? `已连接 ${connection.label.value}` : `已连接 ${connection.label.value} · 延迟 ${speedMs.value} ms`;
});

// 切换局域网目标后清除上一次测速结果
watch(
  () => connection.peerId.value,
  () => {
    speedMs.value = null;
  },
);

onMounted(() => {
  void store.refreshSaved();
  void store.scan();
});

function beginPair(peer: LanPeer): void {
  pairTarget.value = peer;
  code.value = '';
  pairWritable.value = false;
  store.error = null;
}

async function submitPair(): Promise<void> {
  const peer = pairTarget.value;
  const value = code.value.trim().toUpperCase();
  if (!peer || !value) {
    return;
  }
  const ok = await store.pair(peer, value, pairWritable.value);
  if (ok) {
    pairTarget.value = null;
    savedMessage.value = `已连接到 ${peer.name}（${peer.library_name}）`;
    window.setTimeout(() => (savedMessage.value = null), 3000);
  }
}

async function submitManualPair(): Promise<void> {
  const url = manualUrl.value.trim();
  const value = manualCode.value.trim().toUpperCase();
  if (!url || !value) {
    return;
  }
  const peer: LanPeer = {
    id: `manual-${Date.now()}`,
    name: '局域网设备',
    library_name: '',
    url,
    // 目标端仍按 [web].writable 与拆分 token 决定最终权限；这里仅表达请求意图
    writable: true,
    requires_pairing: true,
  };
  const ok = await store.pair(peer, value, manualWritable.value);
  if (ok) {
    manualOpen.value = false;
    manualUrl.value = '';
    manualCode.value = '';
    savedMessage.value = '手动配对成功';
    window.setTimeout(() => (savedMessage.value = null), 3000);
  }
}

async function testSpeed(): Promise<void> {
  if (!isLanConnected.value || testingSpeed.value) {
    return;
  }
  testingSpeed.value = true;
  store.error = null;
  try {
    speedMs.value = (await shell.lanSpeedTest()).latency_ms;
  } catch (e) {
    store.error = ipcMessage(e);
  } finally {
    testingSpeed.value = false;
  }
}

async function connectSaved(peer: LanPeerProfile): Promise<void> {
  await store.connect(peer.id);
}

async function forget(peer: LanPeerProfile): Promise<void> {
  await store.forget(peer.id);
}
</script>

<template>
  <div class="lan-connect">
    <div class="status-row">
      <span class="status-text" :title="statusText">{{ statusText }}</span>
      <div class="status-actions">
        <button v-if="isLanConnected" class="action-btn" :disabled="testingSpeed || store.scanning" @click="testSpeed">
          {{ testingSpeed ? '测速中…' : '测速' }}
        </button>
        <button class="action-btn" :disabled="store.scanning" @click="store.scan">
          <Icon name="refresh" :size="12" />
          {{ store.scanning ? '扫描中…' : '扫描' }}
        </button>
      </div>
    </div>

    <div v-if="store.error" class="error">{{ store.error }}</div>
    <div v-if="savedMessage" class="ok">{{ savedMessage }}</div>

    <div v-if="store.discovered.length > 0" class="group">
      <div class="group-title">发现的设备</div>
      <div v-for="peer in store.discovered" :key="peer.id" class="peer-row">
        <div class="peer-main">
          <div class="peer-name">
            {{ peer.name }}
            <span class="lib">（{{ peer.library_name }}）</span>
          </div>
          <div class="peer-meta">
            {{ peer.url }}
            <span v-if="peer.writable" class="write-badge">允许写入</span>
          </div>
        </div>
        <button class="primary" :disabled="store.connectingId !== null" @click="beginPair(peer)">
          {{ store.connectingId === peer.id ? '配对中…' : '配对连接' }}
        </button>
      </div>
    </div>
    <div v-else-if="!store.scanning" class="empty-state">未发现设备。请确认对方已开启局域网查看，并与本机处于同一网络。</div>

    <div class="manual-row">
      <button class="link-btn" @click="manualOpen = !manualOpen">
        <Icon name="chevronDown" :size="12" :class="{ open: manualOpen }" />
        手动输入地址配对
      </button>
    </div>
    <div v-if="manualOpen" class="pair-box">
      <div class="pair-title">手动配对</div>
      <p class="hint">输入目标设备「局域网」页显示的地址和 8 位配对码。</p>
      <input v-model="manualUrl" placeholder="http://192.168.1.5:27372" autocomplete="off" spellcheck="false" />
      <PairingCodeInput v-model="manualCode" label="8 位配对码" @submit="submitManualPair" />
      <label class="write-request">
        <input v-model="manualWritable" type="checkbox" />
        请求写权限（目标允许时可直接修改对方素材库）
      </label>
      <div class="pair-actions">
        <button class="primary" :disabled="!manualUrl.trim() || !manualCode.trim() || store.connectingId !== null" @click="submitManualPair">
          {{ store.connectingId ? '配对中…' : '连接' }}
        </button>
      </div>
    </div>

    <div v-if="pairTarget" class="pair-box">
      <div class="pair-title">配对 {{ pairTarget.name }}</div>
      <p class="hint">在目标设备生成 8 位配对码后输入。</p>
      <PairingCodeInput v-model="code" label="8 位配对码" @submit="submitPair" />
      <label v-if="pairTarget.writable" class="write-request">
        <input v-model="pairWritable" type="checkbox" />
        请求写权限（目标允许时可直接修改对方素材库）
      </label>
      <div class="pair-actions">
        <button @click="pairTarget = null">取消</button>
        <button class="primary" :disabled="!code.trim() || store.connectingId !== null" @click="submitPair">
          {{ store.connectingId ? '配对中…' : '连接' }}
        </button>
      </div>
    </div>

    <div v-if="store.saved.length > 0" class="group">
      <div class="group-title">已配对设备</div>
      <div v-for="peer in store.saved" :key="peer.id" class="peer-row">
        <div class="peer-main">
          <div class="peer-name">
            {{ peer.name }}
            <span class="lib">（{{ peer.library_name }}）</span>
            <span v-if="connection.isLan.value && connection.peerId.value === peer.id" class="current-badge">当前</span>
          </div>
          <div class="peer-meta">{{ peer.url }}</div>
        </div>
        <div class="row-actions">
          <button :disabled="store.connectingId !== null || (connection.isLan.value && connection.peerId.value === peer.id)" @click="connectSaved(peer)">
            {{ store.connectingId === peer.id ? '连接中…' : '连接' }}
          </button>
          <button class="icon-btn" title="忘记此设备" @click="forget(peer)">
            <Icon name="trash" :size="13" />
          </button>
        </div>
      </div>
    </div>

    <p class="hint footnote">配对码 5 分钟内单次有效；仅建议在可信局域网使用。</p>
  </div>
</template>

<style src="./settings-shared.css"></style>

<style scoped>
.lan-connect {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.status-row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 32px;
}

.status-actions {
  display: flex;
  align-items: center;
  gap: 6px;
  flex: none;
}

.action-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 5px 10px;
  font-size: 12px;
  white-space: nowrap;
}

.status-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--fg-1);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.hint {
  margin: 0;
  color: var(--fg-1);
  font-size: 12px;
  line-height: 1.5;
}
.error {
  color: var(--danger);
  font-size: 12px;
  white-space: pre-wrap;
}
.ok {
  color: var(--accent);
  font-size: 12px;
}
.empty-state {
  padding: 12px;
  border: 1px dashed var(--border);
  border-radius: 7px;
  color: var(--fg-1);
  font-size: 12px;
  line-height: 1.5;
}
.group {
  display: flex;
  flex-direction: column;
}
.group-title {
  padding: 4px 0 3px;
  color: var(--fg-1);
  font-size: 11px;
  font-weight: 600;
}
.peer-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 8px 0;
  border-bottom: 1px solid var(--border);
}
.peer-row:last-child {
  border-bottom: none;
}
.peer-main {
  min-width: 0;
}
.peer-name {
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.lib {
  color: var(--fg-1);
}
.peer-meta {
  margin-top: 2px;
  color: var(--fg-1);
  font-family: monospace;
  font-size: 11px;
  overflow-wrap: anywhere;
}
.write-badge,
.current-badge {
  margin-left: 6px;
  padding: 0 5px;
  border: 1px solid var(--border);
  border-radius: 3px;
  color: var(--fg-1);
  font-family: inherit;
  font-size: 10px;
  font-weight: 400;
}
.row-actions {
  display: flex;
  gap: 5px;
  flex: none;
}
.peer-row button {
  font-size: 12px;
  padding: 5px 10px;
}
.pair-box {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px;
  border: 1px solid var(--border);
  border-radius: 7px;
  background: var(--bg-2);
}
.pair-title {
  font-size: 13px;
  font-weight: 600;
}
.pair-box > input:not([type='checkbox']) {
  width: 100%;
  padding: 7px 9px;
  border: 1px solid var(--border);
  border-radius: 5px;
  background: var(--bg-1);
}
.write-request {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--fg-1);
  font-size: 12px;
}
.write-request input {
  accent-color: var(--accent);
}
.pair-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
.pair-actions button {
  font-size: 12px;
  padding: 5px 12px;
}
.manual-row {
  display: flex;
  justify-content: flex-start;
}
.link-btn {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  padding: 0;
  border: 0;
  background: transparent;
  color: var(--fg-1);
  font-size: 12px;
  cursor: pointer;
}
.link-btn:hover {
  color: var(--accent);
}
.link-btn :deep(svg) {
  transition: transform 0.15s;
}
.link-btn :deep(svg.open) {
  transform: rotate(180deg);
}
.footnote {
  margin: 0;
  padding-top: 2px;
  color: var(--fg-1);
  font-size: 11px;
}
</style>

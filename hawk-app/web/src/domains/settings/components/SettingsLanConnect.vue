<script setup lang="ts">
// 局域网互联（仅 Electron）：发现同网段其他 hawk-app，发起配对请求后等待目标端确认。
// 目标端决定只读/可写权限，源端不经过 hawk-server；token 由主进程保存，本组件只见设备展示信息。
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { useLanStore } from '@/domains/lan';
import { ipcMessage, useConnectionLabel } from '@/domains/remote';
import { shell } from '@/shared/lib/platform';
import Icon from '@/shared/ui/Icon.vue';
import type { LanClient, LanPeer, LanPeerProfile } from '@/shared/types';

const store = useLanStore();
const connection = useConnectionLabel();
const incomingClients = ref<LanClient[]>([]);
let clientTimer: ReturnType<typeof setInterval> | null = null;
const pairPermission = ref<LanPeer | null>(null);
const pairWrite = ref(false);
const savedMessage = ref<string | null>(null);
/** 手动配对兜底：广播/组播被访客网络隔离时，直接输入目标 hawk-app 直连地址。 */
const manualOpen = ref(false);
const manualUrl = ref('');
/** 局域网测速（当前实现为 app/info 往返延迟中位数） */
const testingSpeed = ref(false);
const speedMs = ref<number | null>(null);
const isLanConnected = computed(() => connection.isLan.value);
const currentPeer = computed(() => store.saved.find((peer) => peer.id === connection.peerId.value) ?? null);
/** 当前已连接的设备不再出现在发现列表里，避免重复显示和误点申请。 */
const visibleDiscovered = computed(() => store.discovered.filter((peer) => !(connection.isLan.value && peer.id === connection.peerId.value)));
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

async function refreshClients(): Promise<void> {
  try {
    const { clients } = await shell.lanClients();
    incomingClients.value = clients.filter((client) => client.online);
  } catch {
    // 本机 daemon 尚未就绪/正在重绑时静默；下一轮继续。
  }
}

onMounted(() => {
  void store.refreshSaved();
  void store.scan();
  void refreshClients();
  clientTimer = setInterval(() => void refreshClients(), 4000);
});

onUnmounted(() => {
  if (clientTimer) {
    clearInterval(clientTimer);
  }
});

function openPairPermission(peer: LanPeer): void {
  store.error = null;
  pairWrite.value = false;
  pairPermission.value = peer;
}

async function requestPair(peer: LanPeer, reverseWrite: boolean): Promise<boolean> {
  store.error = null;
  const ok = await store.pair(peer, true, reverseWrite);
  if (ok) {
    savedMessage.value = '已连接到 ' + peer.name + '（' + peer.library_name + '）';
    window.setTimeout(() => (savedMessage.value = null), 3000);
  }
  return ok;
}

async function confirmPairPermission(): Promise<void> {
  const peer = pairPermission.value;
  if (!peer) {
    return;
  }
  pairPermission.value = null;
  const manual = peer.id.startsWith('manual-');
  const ok = await requestPair(peer, pairWrite.value);
  if (ok && manual) {
    manualOpen.value = false;
    manualUrl.value = '';
  }
}
async function submitManualPair(): Promise<void> {
  const url = manualUrl.value.trim();
  if (!url) {
    return;
  }
  const peer: LanPeer = {
    id: `manual-${Date.now()}`,
    name: '局域网设备',
    library_name: '',
    url,
    writable: true,
    requires_pairing: true,
  };
  openPairPermission(peer);
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

    <div v-if="currentPeer" class="current-peer">
      <div class="peer-main">
        <div class="peer-name">
          {{ currentPeer.name }}
          <span class="lib">（{{ currentPeer.library_name }}）</span>
          <span class="current-badge">已连接</span>
        </div>
        <div class="peer-meta">{{ currentPeer.url }}</div>
      </div>
      <button :disabled="store.connectingId !== null" @click="connection.disconnect()">断开</button>
    </div>

    <div v-if="incomingClients.length > 0" class="group">
      <div class="group-title">正在连接的设备</div>
      <div v-for="client in incomingClients" :key="client.key_id" class="peer-row">
        <div class="peer-main">
          <div class="peer-name">
            {{ client.label }}
            <span class="online-badge">在线</span>
          </div>
          <div class="peer-meta">此设备正在通过局域网连接你</div>
        </div>
      </div>
    </div>
    <div v-if="store.error" class="error">{{ store.error }}</div>
    <div v-if="savedMessage" class="ok">{{ savedMessage }}</div>

    <div v-if="visibleDiscovered.length > 0" class="group">
      <div class="group-title">发现的设备</div>
      <div v-for="peer in visibleDiscovered" :key="peer.id" class="peer-row">
        <div class="peer-main">
          <div class="peer-name">
            {{ peer.name }}
            <span class="lib">（{{ peer.library_name }}）</span>
          </div>
          <div class="peer-meta">{{ peer.url }}</div>
        </div>
        <button class="primary" :disabled="store.connectingId !== null" @click="openPairPermission(peer)">
          {{ store.connectingId === peer.id ? '等待对方确认…' : '申请配对' }}
        </button>
      </div>
    </div>
    <div v-else-if="!store.scanning" class="empty-state">{{ isLanConnected ? '未发现其他设备' : '未发现设备' }}</div>

    <div class="manual-row">
      <button class="link-btn" @click="manualOpen = !manualOpen">
        <Icon name="chevronDown" :size="12" :class="{ open: manualOpen }" />
        手动输入地址配对
      </button>
    </div>
    <div v-if="manualOpen" class="pair-box">
      <div class="pair-title">手动配对</div>
      <input v-model="manualUrl" placeholder="http://192.168.1.5:27375" autocomplete="off" spellcheck="false" />
      <div class="pair-actions">
        <button class="primary" :disabled="!manualUrl.trim() || store.connectingId !== null" @click="submitManualPair">
          {{ store.connectingId ? '等待对方确认…' : '申请配对' }}
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

    <Teleport to="body">
      <div v-if="pairPermission" class="pair-perm-mask" @click.self="pairPermission = null">
        <div class="pair-perm-dialog" role="dialog" aria-modal="true" aria-labelledby="pair-perm-title">
          <div id="pair-perm-title" class="pair-perm-title">设备密钥权限</div>
          <p class="pair-perm-copy">将为本机创建「{{ pairPermission.name }}」的设备密钥，用于对方反向访问此素材库。</p>
          <div class="pair-perm-options">
            <label class="pair-perm-option" :class="{ active: !pairWrite }">
              <input v-model="pairWrite" type="radio" :value="false" />
              <span><strong>仅读取</strong></span>
            </label>
            <label class="pair-perm-option" :class="{ active: pairWrite }">
              <input v-model="pairWrite" type="radio" :value="true" />
              <span><strong>可读写</strong></span>
            </label>
          </div>
          <div class="pair-perm-actions">
            <button @click="pairPermission = null">取消</button>
            <button class="primary" @click="confirmPairPermission">申请配对</button>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style src="./settings-shared.css"></style>
<style scoped>
.online-badge {
  margin-left: 6px;
  color: var(--accent);
  font-size: 10px;
  font-weight: 500;
}
.pair-perm-mask {
  position: fixed;
  inset: 0;
  z-index: 230;
  display: grid;
  place-items: center;
  padding: 16px;
  background: rgba(0, 0, 0, 0.52);
}
.pair-perm-dialog {
  width: min(380px, 100%);
  padding: 16px;
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--bg-1);
  box-shadow: 0 14px 40px rgba(0, 0, 0, 0.35);
}
.pair-perm-title {
  margin-bottom: 6px;
  font-size: 14px;
  font-weight: 600;
}
.pair-perm-copy {
  margin: 0 0 12px;
  color: var(--fg-1);
  font-size: 12px;
  line-height: 1.5;
}
.pair-perm-options {
  display: flex;
  flex-direction: column;
  gap: 7px;
}
.pair-perm-option {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  cursor: pointer;
}
.pair-perm-option.active {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 8%, transparent);
}
.pair-perm-option input {
  accent-color: var(--accent);
}
.pair-perm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}
.pair-perm-actions button {
  padding: 5px 10px;
  font-size: 12px;
}
</style>

<style scoped>
.lan-connect {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.current-peer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 9px 11px;
  border: 1px solid var(--accent);
  border-radius: 8px;
  background: color-mix(in srgb, var(--accent) 7%, transparent);
}

.current-peer button {
  flex: none;
  font-size: 12px;
  padding: 4px 10px;
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
.online-badge {
  margin-left: 6px;
  color: var(--accent);
  font-size: 10px;
  font-weight: 500;
}
.pair-perm-mask {
  position: fixed;
  inset: 0;
  z-index: 230;
  display: grid;
  place-items: center;
  padding: 16px;
  background: rgba(0, 0, 0, 0.52);
}
.pair-perm-dialog {
  width: min(380px, 100%);
  padding: 16px;
  border: 1px solid var(--border);
  border-radius: 9px;
  background: var(--bg-1);
  box-shadow: 0 14px 40px rgba(0, 0, 0, 0.35);
}
.pair-perm-title {
  margin-bottom: 6px;
  font-size: 14px;
  font-weight: 600;
}
.pair-perm-copy {
  margin: 0 0 12px;
  color: var(--fg-1);
  font-size: 12px;
  line-height: 1.5;
}
.pair-perm-options {
  display: flex;
  flex-direction: column;
  gap: 7px;
}
.pair-perm-option {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  cursor: pointer;
}
.pair-perm-option.active {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 8%, transparent);
}
.pair-perm-option input {
  accent-color: var(--accent);
}
.pair-perm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}
.pair-perm-actions button {
  padding: 5px 10px;
  font-size: 12px;
}
</style>

<script setup lang="ts">
// 远程访问分区（仅 Electron）：接入自托管 hawk-server（[remote] 段配置）+ 设备列表与会话入口。
// 配置读写/设备列表/会话动作全部经 IPC 委托主进程（切到远程后渲染进程的 API base 已指向会话代理，
// 管理面必须在主进程）。状态自管：保存按钮内联（不占用主组件 footer）；打开期间轮询信令状态。
// 错误条在主组件（v-model:error）；分区切换 v-show 保活（编辑中的字段不丢）。
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useRemoteStore, useConnectionLabel } from '@/domains/remote';

const error = defineModel<string | null>('error', { default: null });
const store = useRemoteStore();
const connection = useConnectionLabel();

const enabled = ref(false);
const url = ref('');
const key = ref('');
const saving = ref(false);

// ---- 本机设备管理（改名 / 远程开关 / 证书轮换） ----
const selfName = ref('');
const renaming = ref(false);
const rotating = ref(false);

onMounted(() => {
  store.startPolling();
  // 表单初值：读当前配置（store.dto 可能已被上次轮询填充）
  void store.refresh().then(() => {
    if (store.dto) {
      enabled.value = store.dto.enabled;
      url.value = store.dto.url;
      key.value = store.dto.key;
    }
  });
});
onUnmounted(() => store.stopPolling());

const statusText = computed(() => {
  const s = store.dto?.status;
  if (!store.dto?.enabled) {
    return null;
  }
  switch (s?.state) {
    case 'connecting':
      return '接入中…';
    case 'online':
      return `已在线（${s.server_url ?? ''}）`;
    case 'error':
      return s.error ?? '接入错误';
    default:
      return null;
  }
});

/** 本机设备（设备列表中 device_id 与信令状态一致的一台） */
const selfDevice = computed(() => store.devices.find((d) => d.device_id === store.dto?.status.device_id) ?? null);

/** 其他设备：在线在前 */
const peerDevices = computed(() =>
  store.devices.filter((d) => d.device_id !== store.dto?.status.device_id).sort((a, b) => Number(b.online) - Number(a.online)),
);

async function save() {
  error.value = null;
  saving.value = true;
  const ok = await store.saveConfig({ enabled: enabled.value, url: url.value.trim(), key: key.value.trim() });
  saving.value = false;
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
</script>

<template>
  <div class="pane">
    <div class="switch-row">
      <div>
        <div class="switch-label">启用远程访问</div>
        <p class="hint">接入自托管 hawk-server 后，可在其他设备上浏览本素材库（只读），也可浏览其他在线设备。</p>
      </div>
      <label class="switch" title="启用远程访问">
        <input v-model="enabled" type="checkbox" />
        <span class="track" />
      </label>
    </div>

    <div v-show="enabled" class="remote-detail">
      <div class="field column">
        <span class="field-label">服务端地址</span>
        <input v-model="url" type="text" placeholder="https://hawk.example.com" autocomplete="off" spellcheck="false" />
      </div>
      <div class="field column">
        <span class="field-label">访问密钥</span>
        <input v-model="key" type="password" placeholder="服务端 access key" autocomplete="off" spellcheck="false" />
      </div>
      <div class="actions-left">
        <button class="primary" :disabled="saving" @click="save">{{ saving ? '保存中…' : '保存并接入' }}</button>
        <span v-if="statusText" class="hint">{{ statusText }}</span>
      </div>

      <template v-if="store.dto?.status.state === 'online'">
        <hr class="divider" />

        <!-- 本机设备：改名 / 远程开关 / 证书轮换 -->
        <div v-if="selfDevice" class="self-device">
          <div class="field column">
            <span class="field-label">本机显示名</span>
            <div class="actions-left">
              <input v-model="selfName" type="text" :placeholder="selfDevice.name" autocomplete="off" />
              <button :disabled="renaming || !selfName.trim()" @click="saveSelfName">改名</button>
            </div>
          </div>
          <div class="switch-row">
            <div>
              <div class="switch-label">允许其他设备访问本库</div>
              <p class="hint">关闭后其他设备的会话请求被自动拒绝。</p>
            </div>
            <label class="switch" title="允许其他设备访问本库">
              <input :checked="selfDevice.allow_remote" type="checkbox" @change="toggleAllowRemote(($event.target as HTMLInputElement).checked)" />
              <span class="track" />
            </label>
          </div>
          <div class="actions-left">
            <button :disabled="rotating" @click="rotate">{{ rotating ? '轮换中…' : '轮换设备证书' }}</button>
            <span class="hint">轮换后旧证书的会话失效（TOFU 锚点更新）</span>
          </div>
        </div>

        <hr class="divider" />

        <!-- 设备列表与会话入口 -->
        <div v-if="connection.isRemote.value" class="actions-left current-remote">
          <span class="hint">当前浏览：{{ connection.label.value }}</span>
          <button @click="disconnect">断开并回到我的库</button>
        </div>
        <div v-if="peerDevices.length === 0" class="hint">暂无其他设备。在另一台设备上运行 hawk 并接入同一服务端即可出现。</div>
        <div v-for="device in peerDevices" :key="device.device_id" class="device-row">
          <span class="dot" :class="{ online: device.online }" />
          <div class="device-info">
            <div class="device-name">{{ device.name }}</div>
            <div class="hint">{{ device.library_name }} · {{ device.platform }}{{ device.allow_remote ? '' : ' · 已关闭访问' }}</div>
          </div>
          <button v-if="device.online && device.allow_remote" class="primary" :disabled="store.connectingId !== null" @click="connect(device.device_id)">
            {{ store.connectingId === device.device_id ? '连接中…' : '浏览素材库' }}
          </button>
          <span v-else class="hint">{{ device.online ? '不可访问' : '离线' }}</span>
        </div>
      </template>
    </div>
  </div>
</template>

<style src="./settings-shared.css"></style>

<style scoped>
.remote-detail {
  display: flex;
  flex-direction: column;
  gap: 12px;
  margin-top: 12px;
}
.actions-left {
  display: flex;
  align-items: center;
  gap: 10px;
}
.divider {
  border: none;
  border-top: 1px solid var(--border, rgba(128, 128, 128, 0.3));
  margin: 4px 0;
}
.device-row {
  display: flex;
  align-items: center;
  gap: 10px;
}
.device-info {
  flex: 1;
  min-width: 0;
}
.device-name {
  font-size: 13px;
}
.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-tertiary, #999);
  flex-shrink: 0;
}
.dot.online {
  background: #4caf50;
}
.current-remote {
  padding: 8px 10px;
  border: 1px solid var(--border, rgba(128, 128, 128, 0.3));
  border-radius: 8px;
}
</style>

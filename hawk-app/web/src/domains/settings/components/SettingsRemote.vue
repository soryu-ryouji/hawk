<script setup lang="ts">
// 远程访问分区（仅 Electron）：接入自托管 hawk-server（应用级 remote.toml）+ 访问密钥管理 +
// 设备列表与会话入口。凭证模型：service-key（池门禁，server 侧 tenant add 生成）准入后，
//  hawk-app key（本机签发，逐把读/写权限）分发给网页版/插件等外部查看者。
// 配置读写/设备列表/密钥管理/会话动作全部经 IPC 委托主进程（切到远程后渲染进程的 API base 已指向
// 会话代理，管理面必须在主进程）。状态自管：保存按钮内联（不占用主组件 footer）；打开期间轮询信令状态。
// 错误条在主组件（v-model:error）；分区切换 v-show 保活（编辑中的字段不丢）。
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useRemoteStore, useConnectionLabel } from '@/domains/remote';
import type { RemoteKeyCreated } from '@/shared/types';

const error = defineModel<string | null>('error', { default: null });
const store = useRemoteStore();
const connection = useConnectionLabel();

const enabled = ref(false);
const url = ref('');
const serviceKey = ref('');
const writable = ref(false);
const saving = ref(false);

// ---- 本机设备管理（改名 / 远程开关 / 证书轮换） ----
const selfName = ref('');
const renaming = ref(false);
const rotating = ref(false);

// ---- 访问密钥管理 ----
const keyLabel = ref('');
const keyRead = ref(true);
const keyWrite = ref(false);
const keyCustom = ref('');
const creating = ref(false);
/** 刚签发的密钥（明文只在创建响应里，展示到用户关闭为止） */
const createdKey = ref<RemoteKeyCreated | null>(null);
const copied = ref('');

onMounted(() => {
  store.startPolling();
  // 表单初值：读当前配置（store.dto 可能已被上次轮询填充）
  void store.refresh().then(() => {
    if (store.dto) {
      enabled.value = store.dto.enabled;
      url.value = store.dto.url;
      serviceKey.value = store.dto.service_key;
      writable.value = store.dto.writable;
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
  const ok = await store.saveConfig({
    enabled: enabled.value,
    url: url.value.trim(),
    service_key: serviceKey.value.trim(),
    writable: writable.value,
  });
  saving.value = false;
  if (!ok) {
    error.value = store.error;
  }
}

// ---- 访问密钥动作 ----

async function createKey() {
  error.value = null;
  createdKey.value = null;
  creating.value = true;
  const created = await store.createKey({
    label: keyLabel.value.trim() || undefined,
    read: keyRead.value,
    write: keyWrite.value,
    custom: keyCustom.value.trim() || undefined,
  });
  creating.value = false;
  if (!created) {
    error.value = store.error;
    return;
  }
  createdKey.value = created;
  keyLabel.value = '';
  keyCustom.value = '';
  keyWrite.value = false;
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
  return [`服务器：${store.dto?.url ?? ''}`, `服务密钥：${store.dto?.service_key ?? ''}`, `访问密钥：${key}`].join('\n');
}

async function toggleKeyPerm(id: string, perm: 'read' | 'write', value: boolean) {
  error.value = null;
  const ok = await store.patchKey(id, { [perm]: value });
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
</script>

<template>
  <div class="pane">
    <div class="switch-row">
      <div>
        <div class="switch-label">启用远程访问</div>
        <p class="hint">接入自托管 hawk-server 后，可在其他设备上浏览本素材库（只读），也可浏览其他在线设备。接入配置跨素材库共享。</p>
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
        <span class="field-label">服务密钥（service-key）</span>
        <input v-model="serviceKey" type="password" placeholder="hawk-server tenant add 生成的 hs_ 密钥" autocomplete="off" spellcheck="false" />
        <p class="hint">池门禁凭证：接入后本机可被发现，也可浏览池内其他设备。服务密钥不对外分发——分发给查看者的是下面的访问密钥。</p>
      </div>
      <div class="switch-row">
        <div>
          <div class="switch-label">允许广域网保存素材</div>
          <p class="hint">允许持有服务端写入密钥的浏览器插件向本库导入图片、新建文件夹。与局域网写权限独立；不开放删除、改标签等操作。</p>
        </div>
        <label class="switch" title="允许广域网会话导入素材">
          <input v-model="writable" type="checkbox" />
          <span class="track" />
        </label>
      </div>

      <div class="actions-left">
        <button class="primary" :disabled="saving" @click="save">{{ saving ? '保存中…' : '保存并接入' }}</button>
        <span v-if="statusText" class="hint">{{ statusText }}</span>
      </div>

      <hr class="divider" />

      <!-- 访问密钥管理：分发给网页版/插件等外部查看者（与池门禁的 service-key 不同物） -->
      <div class="keys">
        <div class="switch-label">访问密钥</div>
        <p class="hint">每把密钥独立读/写权限，可随时吊销；查看者需同时提供服务密钥与访问密钥。密钥明文只在签发时显示一次。</p>

        <!-- 签发结果：明文展示一次 -->
        <div v-if="createdKey" class="created-key">
          <code class="created-key-code">{{ createdKey.key }}</code>
          <div class="actions-left">
            <button @click="copyText(createdKey!.key, 'key')">{{ copied === 'key' ? '已复制' : '复制密钥' }}</button>
            <button @click="copyText(shareText(createdKey!.key), 'share')">{{ copied === 'share' ? '已复制' : '复制分享串' }}</button>
            <button @click="createdKey = null">我已保存</button>
          </div>
          <p class="hint">{{ createdKey.synced ? '已同步到服务端。' : '远程未在线，同步会在接入后自动重试。' }}此后无法再查看明文。</p>
        </div>

        <!-- 签发表单 -->
        <div class="key-form">
          <input v-model="keyLabel" type="text" placeholder="标签（如 iPhone 插件）" autocomplete="off" />
          <input v-model="keyCustom" type="text" placeholder="自定义密钥（留空自动生成）" autocomplete="off" spellcheck="false" />
          <div class="actions-left">
            <label class="perm"><input v-model="keyRead" type="checkbox" :disabled="keyRead && !keyWrite" /> 可读</label>
            <label class="perm"><input v-model="keyWrite" type="checkbox" :disabled="!keyRead && keyWrite" /> 可写</label>
            <button class="primary" :disabled="creating || (!keyRead && !keyWrite)" @click="createKey">{{ creating ? '签发中…' : '签发' }}</button>
          </div>
          <p v-if="keyWrite" class="hint">携带写权限的自定义密钥至少 12 个字符；写权限允许导入素材与新建文件夹。</p>
        </div>

        <!-- 已有密钥列表 -->
        <div v-if="store.keys.length === 0" class="hint">尚未签发访问密钥。</div>
        <div v-for="k in store.keys" :key="k.id" class="key-row">
          <div class="key-info">
            <div class="key-label">
              {{ k.label || '未命名' }} <code class="key-prefix">{{ k.prefix }}…</code>
            </div>
          </div>
          <label class="perm" title="只读浏览">
            <input
              type="checkbox"
              :checked="k.read"
              :disabled="k.read && !k.write"
              @change="toggleKeyPerm(k.id, 'read', ($event.target as HTMLInputElement).checked)"
            />
            读
          </label>
          <label class="perm" title="广域网导入素材/新建文件夹">
            <input
              type="checkbox"
              :checked="k.write"
              :disabled="!k.read && k.write"
              @change="toggleKeyPerm(k.id, 'write', ($event.target as HTMLInputElement).checked)"
            />
            写
          </label>
          <button class="key-revoke" @click="revokeKey(k.id, k.label)">吊销</button>
        </div>
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
        <div class="switch-row">
          <div>
            <div class="switch-label">中继回退</div>
            <p class="hint">直连失败时询问是否经服务端中继继续（素材流量经过你的服务器带宽，内容端到端加密）。</p>
          </div>
          <label class="switch" title="直连失败时经服务端中继回退">
            <input :checked="store.relayFallback" type="checkbox" @change="store.setRelayFallback(($event.target as HTMLInputElement).checked)" />
            <span class="track" />
          </label>
        </div>

        <div v-if="peerDevices.length === 0" class="hint">暂无其他设备。在另一台设备上运行 hawk 并接入同一服务端即可出现。</div>
        <div v-for="device in peerDevices" :key="device.device_id" class="device-row">
          <span class="dot" :class="{ online: device.online }" />
          <div class="device-info">
            <div class="device-name">{{ device.name }}</div>
            <div class="hint">
              {{ device.library_name }} · {{ device.platform }}{{ device.allow_remote ? '' : ' · 已关闭访问' }}{{ device.allow_write ? ' · 允许远程写入' : '' }}
            </div>
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

/* ---- 访问密钥管理 ---- */
.keys {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.created-key {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px;
  border: 1px solid var(--accent);
  border-radius: 8px;
}
.created-key-code {
  font-family: ui-monospace, monospace;
  font-size: 13px;
  word-break: break-all;
  user-select: all;
}
.key-form {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.key-row {
  display: flex;
  align-items: center;
  gap: 12px;
}
.key-info {
  flex: 1;
  min-width: 0;
}
.key-label {
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.key-prefix {
  font-family: ui-monospace, monospace;
  font-size: 11px;
  color: var(--text-tertiary, #999);
}
.perm {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: var(--fg-1);
  flex-shrink: 0;
}
.key-revoke {
  flex-shrink: 0;
  color: var(--danger);
}
</style>

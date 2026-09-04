<script setup lang="ts">
// 局域网查看分区（仅 Electron）：开关/端口/token/本机地址，按库隔离存于 .hawk/config.toml 的 [web] 段。
// 配置读写直连 daemon REST（GET/PUT /api/v1/app/lan，admin 限定），保存 = daemon 写配置并热重绑监听
// （不重启进程），绑定失败 daemon 侧自动回滚并返回错误；本机地址列表经 preload（主进程网卡信息）。
// 状态自管（加载/校验/保存）；主组件 footer 的保存按钮经 expose 的 save() 委托到这里，
// 分区切换用 v-show 保活（编辑中的字段不丢）。错误条在主组件（v-model:error）。
import { computed, onMounted, ref } from 'vue';
import { useClipboard } from '@vueuse/core';
import { api } from '../api/endpoints';
import { errorText } from '../stores/util';
import { useLibraryStore } from '../stores/library';
import { shell } from '../platform';
import Icon from './Icon.vue';

const store = useLibraryStore();
const error = defineModel<string | null>('error', { default: null });

const loading = ref(true);
const saving = ref(false);
const enabled = ref(false);
const port = ref('27372');
const token = ref('');
const writable = ref(false);
const separate = ref(false);
const writeToken = ref('');
const addresses = ref<string[]>([]);
const { copy: copyText } = useClipboard({ legacy: true });

onMounted(async () => {
  try {
    const s = await api.appLan();
    enabled.value = s.enabled;
    port.value = String(s.port);
    token.value = s.token;
    writable.value = s.writable;
    separate.value = s.separate_write_token;
    writeToken.value = s.write_token;
    addresses.value = await shell.lanAddresses();
  } catch (e) {
    error.value = `读取设置失败：${errorText(e)}`;
  } finally {
    loading.value = false;
  }
});

// ---- 端口校验：纯数字且 1–65535；不合法就地标红提示，保存拦截 ----
const portValid = computed(() => /^\d+$/.test(port.value.trim()) && Number(port.value) >= 1 && Number(port.value) <= 65535);
const PORT_ERROR = '端口须为 1–65535 之间的数字';

/** 保存用的端口值：合法取解析值；未启用局域网时输入框不可见，静默回退默认端口 */
function portValue() {
  return portValid.value ? Number(port.value.trim()) : 27372;
}

/** 生成随机 token（32 字节 hex） */
function randomToken() {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return [...bytes].map((b) => b.toString(16).padStart(2, '0')).join('');
}

function regenerateToken() {
  token.value = randomToken();
}

function regenerateWriteToken() {
  writeToken.value = randomToken();
}

/** 切换拆分模式：开启且可写 token 为空时自动签发一个 */
function toggleSeparate(on: boolean) {
  separate.value = on;
  if (on && !writeToken.value.trim()) {
    writeToken.value = randomToken();
  }
}

/** 复制文本并给全局 toast 反馈（token/访问地址，发给手机侧粘贴用） */
function copy(value: string) {
  void copyText(value);
  store.showToast('已复制到剪贴板');
}

/** 保存 [web] 配置（主组件 footer 保存按钮的委托入口）：成功返回 true（主组件据此关闭对话框） */
async function save(): Promise<boolean> {
  if (saving.value) {
    return false;
  }
  if (enabled.value && !token.value.trim()) {
    error.value = '启用局域网查看需要填写访问 token';
    return false;
  }
  if (enabled.value && writable.value && separate.value && !writeToken.value.trim()) {
    error.value = '拆分只读/可写 token 需要填写可写 token';
    return false;
  }
  if (enabled.value && !portValid.value) {
    error.value = PORT_ERROR;
    return false;
  }
  saving.value = true;
  error.value = null;
  try {
    await api.saveAppLan({
      enabled: enabled.value,
      port: portValue(),
      token: token.value.trim(),
      writable: writable.value,
      separate_write_token: separate.value,
      write_token: writeToken.value.trim(),
    });
    // 成功：LAN 监听已热重绑（daemon 侧确认收敛，失败已自动回滚）
    return true;
  } catch (e) {
    error.value = `应用失败：${errorText(e)}`;
    return false;
  } finally {
    saving.value = false;
  }
}

/** footer 按钮禁用状态（加载/保存中） */
function busy(): boolean {
  return loading.value || saving.value;
}

defineExpose({ save, busy });
</script>

<template>
  <div class="pane">
    <div class="switch-row">
      <div>
        <div class="switch-label">启用局域网 web 查看</div>
        <p class="hint">同一局域网的设备可用浏览器只读浏览本素材库。</p>
      </div>
      <label class="switch" title="启用局域网 web 查看">
        <input v-model="enabled" type="checkbox" />
        <span class="track" />
      </label>
    </div>

    <div v-if="loading" class="hint">读取设置中…</div>
    <template v-else>
      <!-- 未启用时收起细节字段，减少噪音 -->
      <div v-show="enabled" class="lan-detail">
        <div class="switch-row">
          <div>
            <div class="switch-label">允许修改素材库</div>
            <p class="hint">开启后查看端可上传、删除、修改素材；请谨慎签发可写 token。</p>
          </div>
          <label class="switch" title="允许局域网查看端执行写操作">
            <input v-model="writable" type="checkbox" />
            <span class="track" />
          </label>
        </div>

        <!-- token 模式：二合一（单 token 读写）/拆分（只读 token + 可写 token），
             类似双频 WiFi 的合频/分频；仅写权限开启后有意义 -->
        <div v-if="writable" class="switch-row">
          <div>
            <div class="switch-label">拆分只读与可写 token</div>
            <p class="hint">关闭时访问 token 兼具读写权限；开启后访问 token 仅可浏览，修改需另签发可写 token。</p>
          </div>
          <label class="switch" title="拆分只读 token 与可写 token">
            <input
              :checked="separate"
              type="checkbox"
              @change="toggleSeparate(($event.target as HTMLInputElement).checked)"
            />
            <span class="track" />
          </label>
        </div>

        <div class="field column">
          <span class="field-label">端口</span>
          <input
            v-model="port"
            class="port-input"
            :class="{ invalid: !portValid }"
            type="text"
            inputmode="numeric"
            autocomplete="off"
            spellcheck="false"
            placeholder="1 – 65535"
          />
          <p v-if="!portValid" class="field-error">{{ PORT_ERROR }}</p>
        </div>

        <div class="field column">
          <span class="field-label">访问 token{{ separate ? '（只读）' : '' }}</span>
          <div class="token-row">
            <input v-model="token" type="text" autocomplete="off" spellcheck="false" />
            <button class="icon-btn" title="复制 token" @click="copy(token)">
              <Icon name="copy" :size="13" />
            </button>
            <button title="重新生成随机 token" @click="regenerateToken">重新生成</button>
          </div>
        </div>

        <div v-if="separate" class="field column">
          <span class="field-label">可写 token</span>
          <div class="token-row">
            <input v-model="writeToken" type="text" autocomplete="off" spellcheck="false" />
            <button class="icon-btn" title="复制可写 token" @click="copy(writeToken)">
              <Icon name="copy" :size="13" />
            </button>
            <button title="重新生成随机 token" @click="regenerateWriteToken">重新生成</button>
          </div>
        </div>

        <div class="field column">
          <span class="field-label">访问地址</span>
          <ul class="addrs">
            <li v-for="ip in addresses" :key="ip">
              <a
                :href="portValid ? `http://${ip}:${port.trim()}` : undefined"
                target="_blank"
                rel="noreferrer"
                >http://{{ ip }}:{{ port }}</a
              >
              <button
                class="icon-btn"
                :disabled="!portValid"
                title="复制地址"
                @click="copy(`http://${ip}:${port.trim()}`)"
              >
                <Icon name="copy" :size="13" />
              </button>
            </li>
            <li v-if="addresses.length === 0" class="hint">未检测到局域网 IPv4 地址（检查本机网络连接）</li>
          </ul>
          <p class="hint">
            在浏览器打开地址并输入访问 token 即可查看；首次启用时 Windows 可能弹出防火墙授权框，请选择「允许」。
          </p>
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

.port-input.invalid {
  border-color: var(--danger);
}

.token-row {
  display: flex;
  align-items: center;
  gap: 6px;
}

.token-row input {
  flex: 1;
  min-width: 0;
  padding: 5px 8px;
  font-family: monospace;
  font-size: 12px;
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
</style>

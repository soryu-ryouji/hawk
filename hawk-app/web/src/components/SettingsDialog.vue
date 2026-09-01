<script setup lang="ts">
// 设置面板：左侧导航（外观/局域网）+ 右侧内容的两栏结构（窄屏折叠为顶部横向页签；无 shell 的
// 移动端只有「外观」一个分区，导航不渲染）。
// - 缩略图尺寸滑杆：实时生效，所有端可用（含局域网浏览器触屏端）。
// - 局域网查看（仅 Electron）：开关/端口/token/本机地址，按库隔离存于 .hawk/config.toml 的 [web] 段；
//   保存 = 主进程写配置，daemon 热重绑监听（不重启进程），主进程轮询确认收敛，
//   失败自动写回旧配置回滚并弹错。
// 交互要点：
// - 遮罩「按下与抬起都落在遮罩上」才关闭：在端口输入框里拖动选择文本、拖动滑杆时滑出面板松开，
//   click 事件落在 mousedown/mouseup 目标的共同祖先（遮罩）上，按 click.self 判定会误关面板丢失未保存
//   的配置——改用 pointerdown/pointerup 配对判定，从面板内开始的拖拽不再触发关闭。
// - Esc 关闭（捕获阶段拦截并阻断全局快捷键；IME 组合态已被 main.ts 更早的捕获监听拦下）。
// - 打开期间挂 body.dialog-open 挂起窗口拖拽区（同 ContextMenu 的 body.menu-open）：Electron 的
//   -webkit-app-region: drag 由 OS 命中测试优先消费，不禁用的话点遮罩盖住的标题栏会变成拖动窗口。
// - 端口为纯文本输入（type=number 的原生步进按钮易误触且样式不可控），合法性就地为红色边框 +
//   提示文案，保存时拦截。
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useClipboard, useEventListener } from '@vueuse/core';
import { useLibraryStore } from '../stores/library';
import Icon from './Icon.vue';
import type { LanSettings } from '../types';

const emit = defineEmits<{ close: []; logout: [] }>();

const store = useLibraryStore();
const hasShell = !!window.hawkShell;
const loading = ref(true);
const saving = ref(false);
const error = ref<string | null>(null);
const enabled = ref(false);
const port = ref('27372');
const token = ref('');
const writable = ref(false);
const separate = ref(false);
const writeToken = ref('');
const addresses = ref<string[]>([]);
const { copy: copyText } = useClipboard({ legacy: true });

/** 当前分区：外观 / 局域网（Electron）/ 连接（局域网 web 端，含 token 注销） */
const section = ref<'appearance' | 'lan' | 'connection'>('appearance');

onMounted(async () => {
  document.body.classList.add('dialog-open');
  if (!hasShell) {
    // 浏览器触屏端：无局域网设置可加载（滑杆段实时生效，无需加载态）
    loading.value = false;
    return;
  }
  try {
    const shell = window.hawkShell;
    if (!shell?.getLanSettings) {
      throw new Error('preload 无 getLanSettings 通道');
    }
    const s: LanSettings = await shell.getLanSettings();
    enabled.value = s.enabled;
    port.value = String(s.port);
    token.value = s.token;
    writable.value = s.writable;
    separate.value = s.separateWriteToken;
    writeToken.value = s.writeToken;
    addresses.value = s.addresses;
  } catch (e) {
    // 多见于主进程/preload 未随新版本重启（dev.mjs 不监听主进程文件）
    error.value = `读取设置失败：${e instanceof Error ? e.message : String(e)}（请完全重启 hawk 后重试）`;
  } finally {
    loading.value = false;
  }
});

onUnmounted(() => {
  document.body.classList.remove('dialog-open');
});

// ---- 遮罩关闭：按下与抬起都落在遮罩上才关（面板内开始的拖拽出面板松开不误关） ----
let downOnMask = false;

function onMaskDown(e: PointerEvent) {
  downOnMask = e.target === e.currentTarget;
}

function onMaskUp(e: PointerEvent) {
  if (downOnMask && e.target === e.currentTarget) {
    emit('close');
  }
  downOnMask = false;
}

// Esc 关闭：捕获阶段处理并阻断冒泡，避免全局快捷键（关预览/菜单）跟着触发
useEventListener(
  window,
  'keydown',
  (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      emit('close');
    }
  },
  { capture: true },
);

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

/** 缩略图尺寸步进（滑杆 ± 按钮） */
function stepThumb(delta: number) {
  store.thumbSize = Math.min(280, Math.max(120, store.thumbSize + delta));
}

async function save() {
  if (saving.value) {
    return;
  }
  if (enabled.value && !token.value.trim()) {
    error.value = '启用局域网查看需要填写访问 token';
    return;
  }
  if (enabled.value && writable.value && separate.value && !writeToken.value.trim()) {
    error.value = '拆分只读/可写 token 需要填写可写 token';
    return;
  }
  if (enabled.value && !portValid.value) {
    error.value = PORT_ERROR;
    section.value = 'lan';
    return;
  }
  saving.value = true;
  error.value = null;
  try {
    const shell = window.hawkShell;
    if (!shell?.saveLanSettings) {
      throw new Error('preload 无 saveLanSettings 通道');
    }
    const res = await shell.saveLanSettings({
      enabled: enabled.value,
      port: portValue(),
      token: token.value.trim(),
      writable: writable.value,
      separateWriteToken: separate.value,
      writeToken: writeToken.value.trim(),
    });
    if (!res.ok) {
      error.value = res.error ?? '应用失败';
    } else {
      // 成功：LAN 监听已热重绑（主进程轮询 app/info 确认收敛，无重启），关闭本对话框
      emit('close');
    }
  } catch (e) {
    error.value = `应用失败：${e instanceof Error ? e.message : String(e)}（请完全重启 hawk 后重试）`;
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <Teleport to="body">
    <div class="mask" @pointerdown="onMaskDown" @pointerup="onMaskUp">
      <div class="dialog" role="dialog" aria-modal="true" aria-label="设置">
        <header class="dialog-head">
          <span class="dialog-title">设置</span>
          <button class="icon-btn" title="关闭 (Esc)" @click="emit('close')">
            <Icon name="close" :size="14" />
          </button>
        </header>

        <div class="dialog-main">
          <!-- 左侧导航：桌面（外观/局域网）与局域网 web 端（外观/连接）均为双分区；窄屏折叠为顶部横向页签 -->
          <nav class="nav">
            <button class="nav-item" :class="{ active: section === 'appearance' }" @click="section = 'appearance'">
              外观
            </button>
            <button
              v-if="hasShell"
              class="nav-item"
              :class="{ active: section === 'lan' }"
              @click="section = 'lan'"
            >
              局域网
            </button>
            <button
              v-else
              class="nav-item"
              :class="{ active: section === 'connection' }"
              @click="section = 'connection'"
            >
              连接
            </button>
          </nav>

          <!-- 外观：所有端可用 -->
          <div v-if="section === 'appearance'" class="pane">
            <div class="field">
              <span class="field-label">缩略图尺寸</span>
              <span class="slider-val">{{ store.thumbSize }}</span>
            </div>
            <div class="slider-row">
              <button title="缩小" @click="stepThumb(-8)">−</button>
              <input v-model.number="store.thumbSize" type="range" min="120" max="280" step="8" />
              <button title="放大" @click="stepThumb(8)">＋</button>
            </div>
          </div>

          <!-- 局域网：依赖 Electron preload 通道，浏览器 web 端导航不渲染该项 -->
          <div v-else-if="section === 'lan'" class="pane">
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

          <!-- 连接（仅局域网 web 端）：当前访问级别 + token 注销（换身份重新输入） -->
          <div v-else-if="section === 'connection'" class="pane">
            <div class="field">
              <span class="field-label">访问级别</span>
              <span>{{ store.viewerMode ? '只读' : '可读写' }}</span>
            </div>
            <p class="hint">
              本浏览器已记住当前 token；切换只读/可写身份时注销后重新输入另一个 token 即可。
            </p>
            <div class="actions-left">
              <button class="danger" @click="emit('logout')">注销 token</button>
            </div>
          </div>
        </div>

        <div v-if="error" class="dialog-error">{{ error }}</div>

        <footer class="actions">
          <button :disabled="saving || loading" @click="emit('close')">{{ hasShell ? '取消' : '关闭' }}</button>
          <button v-if="hasShell" class="primary" :disabled="saving || loading" @click="save">
            {{ saving ? '保存中…' : '保存' }}
          </button>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 170;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.5);
}

/* 标题栏/底部按钮常驻，只有内容区滚动。
   高度固定（钳制视口）：分区切换/开关展开细节/错误条出现只改变内容区滚动，
   面板尺寸不变，避免界面跳跃 */
.dialog {
  width: min(560px, calc(100vw - 32px));
  height: min(520px, 86vh);
  display: flex;
  flex-direction: column;
  border-radius: 10px;
  background: var(--bg-1);
  border: 1px solid var(--border);
  box-shadow: 0 12px 40px rgba(0, 0, 0, 0.4);
  overflow: hidden;
}

.dialog-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px 10px 16px;
  border-bottom: 1px solid var(--border);
}

.dialog-title {
  font-weight: 600;
}

/* ---- 左导航 + 右内容；窄屏折叠为顶部横向页签 ---- */
.dialog-main {
  flex: 1;
  min-height: 0;
  display: flex;
}

.nav {
  flex: none;
  width: 112px;
  padding: 10px 8px;
  border-right: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.nav-item {
  text-align: left;
  border: none;
  border-radius: 6px;
  background: transparent;
  color: var(--fg-1);
  padding: 6px 10px;
}

.nav-item:hover {
  background: var(--bg-3);
  color: var(--fg-0);
}

.nav-item.active {
  background: var(--bg-3);
  color: var(--fg-0);
  font-weight: 600;
}

.pane {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

/* ---- 局域网开关行 ---- */
.switch-row {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}

.switch-label {
  color: var(--fg-0);
}

.hint {
  margin: 4px 0 0;
  font-size: 12px;
  color: var(--fg-1);
}

.switch {
  position: relative;
  display: inline-block;
  width: 34px;
  height: 20px;
  flex: none;
  margin-top: 1px;
}

.switch input {
  position: absolute;
  inset: 0;
  margin: 0;
  border: none;
  opacity: 0;
  cursor: pointer;
}

.switch .track {
  position: absolute;
  inset: 0;
  border-radius: 10px;
  background: var(--bg-3);
  border: 1px solid var(--border);
  transition: background 0.15s, border-color 0.15s;
  pointer-events: none;
}

.switch .track::after {
  content: '';
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--fg-1);
  transition: transform 0.15s, background 0.15s;
}

.switch input:checked + .track {
  background: var(--accent);
  border-color: var(--accent);
}

.switch input:checked + .track::after {
  transform: translateX(14px);
  background: #fff;
}

.switch input:focus-visible + .track {
  outline: 2px solid var(--accent);
  outline-offset: 1px;
}

/* ---- 字段：标签置顶的块状排布 ---- */
.lan-detail {
  display: flex;
  flex-direction: column;
  gap: 14px;
  margin-top: 4px;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}

.field {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.field.column {
  flex-direction: column;
  align-items: stretch;
  gap: 6px;
}

.field-label {
  color: var(--fg-0);
}

.port-input.invalid {
  border-color: var(--danger);
}

.field-error {
  margin: 0;
  font-size: 12px;
  color: var(--danger);
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

.icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  padding: 0;
  flex: none;
  color: var(--fg-1);
}

/* ---- 缩略图滑杆 ---- */
.slider-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.slider-row input[type='range'] {
  flex: 1;
  padding: 0;
  border: none;
  background: transparent;
}

.slider-val {
  margin-left: auto;
  color: var(--fg-1);
  font-variant-numeric: tabular-nums;
}

.dialog-error {
  padding: 8px 16px;
  border-top: 1px solid var(--border);
  color: var(--danger);
  font-size: 12px;
}

.actions-left {
  display: flex;
  margin-top: 8px;
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--border);
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

/* 窄屏（手机竖屏）：导航折叠为顶部横向页签，内容占满剩余宽度 */
@media (max-width: 520px) {
  .dialog-main {
    flex-direction: column;
  }

  .nav {
    width: auto;
    flex-direction: row;
    border-right: none;
    border-bottom: 1px solid var(--border);
    padding: 8px 10px;
  }

  .nav-item {
    flex: 1;
    text-align: center;
  }
}
</style>

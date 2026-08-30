<script setup lang="ts">
// 设置面板：缩略图尺寸（实时生效，所有端可用）；局域网 web 查看开关/端口/token（仅 Electron，
// 按库隔离存于 .hawk/config.toml 的 [web] 段；保存 = 主进程写配置并重启 hawk-server，失败自动回滚并弹错）。
// 移动端（浏览器触屏）可打开本面板调整缩略图尺寸，但无 hawkShell，局域网设置段不渲染。
import { onMounted, ref } from 'vue';
import { useLibraryStore } from '../stores/library';
import type { LanSettings } from '../types';

const emit = defineEmits<{ close: [] }>();

const store = useLibraryStore();
const hasShell = !!window.hawkShell;
const loading = ref(true);
const saving = ref(false);
const error = ref<string | null>(null);
const enabled = ref(false);
const port = ref(27372);
const token = ref('');
const addresses = ref<string[]>([]);

onMounted(async () => {
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
    port.value = s.port;
    token.value = s.token;
    addresses.value = s.addresses;
  } catch (e) {
    // 多见于主进程/preload 未随新版本重启（dev.mjs 不监听主进程文件）
    error.value = `读取设置失败：${e instanceof Error ? e.message : String(e)}（请完全重启 hawk 后重试）`;
  } finally {
    loading.value = false;
  }
});

/** 重新生成随机访问 token（32 字节 hex） */
function regenerate() {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  token.value = [...bytes].map((b) => b.toString(16).padStart(2, '0')).join('');
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
  saving.value = true;
  error.value = null;
  try {
    const shell = window.hawkShell;
    if (!shell?.saveLanSettings) {
      throw new Error('preload 无 saveLanSettings 通道');
    }
    const res = await shell.saveLanSettings({
      enabled: enabled.value,
      port: Number(port.value) || 27372,
      token: token.value.trim(),
    });
    if (!res.ok) {
      error.value = res.error ?? '应用失败';
    } else {
      // 成功：server 已重启就绪（新地址/token 经 server-started 事件推送到 App 原地重载数据），关闭本对话框
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
    <div class="mask" @click.self="emit('close')">
      <div class="dialog" role="dialog" aria-modal="true">
        <div class="title">设置</div>

        <section>
          <div class="section-title">缩略图尺寸</div>
          <div class="slider-row">
            <button title="缩小" @click="stepThumb(-8)">−</button>
            <input v-model.number="store.thumbSize" type="range" min="120" max="280" step="8" />
            <button title="放大" @click="stepThumb(8)">＋</button>
            <span class="slider-val">{{ store.thumbSize }}</span>
          </div>
        </section>

        <!-- 远程设置：依赖 Electron preload 的局域网通道，移动端（浏览器触屏）不渲染 -->
        <template v-if="hasShell">
          <div v-if="loading" class="hint">加载中…</div>
          <template v-else>
            <section>
              <div class="section-title">局域网查看</div>
              <label class="row">
                <input v-model="enabled" type="checkbox" />
                <span>启用局域网 web 查看（只读）</span>
              </label>
              <p class="hint">其他设备通过浏览器访问本素材库；查看端仅可浏览，不能修改素材库。</p>
            </section>

            <section>
              <div class="section-title">端口</div>
              <input v-model.number="port" type="number" min="1" max="65535" :disabled="!enabled" />
            </section>

            <section>
              <div class="section-title">访问 token</div>
              <div class="token-row">
                <input v-model="token" type="text" :disabled="!enabled" autocomplete="off" spellcheck="false" />
                <button :disabled="!enabled" @click="regenerate">重新生成</button>
              </div>
              <p class="hint">局域网设备打开下方地址后输入该 token 即可查看。</p>
            </section>

            <section>
              <div class="section-title">局域网访问地址（其他设备用浏览器打开）</div>
              <ul class="addrs">
                <li v-for="ip in addresses" :key="ip">
                  <a :href="`http://${ip}:${port}`" target="_blank" rel="noreferrer">http://{{ ip }}:{{ port }}</a>
                </li>
                <li v-if="addresses.length === 0" class="hint">未检测到局域网 IPv4 地址（检查本机网络连接）</li>
              </ul>
              <p class="hint">地址随上方端口实时变化；打开后输入访问 token 即可查看素材库。</p>
              <p v-if="!enabled" class="hint">启用「局域网查看」后以上地址生效。</p>
              <p class="hint">首次启用时 Windows 可能弹出防火墙授权框，请选择「允许」。</p>
            </section>

            <div v-if="error" class="error">{{ error }}</div>
          </template>
        </template>

        <div class="actions">
          <button :disabled="saving || loading" @click="emit('close')">{{ hasShell ? '取消' : '关闭' }}</button>
          <button v-if="hasShell" class="primary" :disabled="saving || loading" @click="save">
            {{ saving ? '应用中…（正在重启服务）' : '保存并重启服务' }}
          </button>
        </div>
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

.dialog {
  /* 手机竖屏也放得下（移动端可打开本面板） */
  width: min(420px, calc(100vw - 32px));
  max-height: 84vh;
  overflow-y: auto;
  padding: 18px 20px;
  border-radius: 8px;
  background: var(--bg-2);
  border: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 14px;
}

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
  min-width: 32px;
  text-align: right;
  color: var(--fg-1);
  font-variant-numeric: tabular-nums;
}

.title {
  font-weight: 600;
}

section {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.section-title {
  font-size: 12px;
  color: var(--fg-1);
}

.row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.token-row {
  display: flex;
  gap: 8px;
}

.token-row input {
  flex: 1;
  padding: 6px 8px;
  font-family: monospace;
}

.addrs {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.addrs a {
  color: var(--accent);
  text-decoration: none;
}

.hint {
  margin: 0;
  font-size: 12px;
  color: var(--fg-1);
}

.error {
  color: var(--danger);
  font-size: 12px;
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}
</style>

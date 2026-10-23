<script setup lang="ts">
// 本地服务（仅 Electron，设置面板「网络」分区的首块）：本地 daemon 的 API 端口。
// 端口的唯一所有者是壳层——daemon 严格绑定这个端口，绑不上由启动屏询问换端口（不静默漂移），
// 浏览器插件才能按固定段（27371–27400）探活并零配置接入。改端口需要重启 daemon（前端切启动屏）。
import { computed, onMounted, ref } from 'vue';
import { shell } from '@/shared/lib/platform';

const current = ref<number | null>(null);
/** 自动发现候选段（来自主进程，跨仓库契约见 app-config.ts 的 API_PORT_RANGE） */
const range = ref<readonly [number, number]>([27371, 27400]);
const input = ref('');
const saving = ref(false);
const error = ref('');
const notice = ref('');

onMounted(async () => {
  const info = await shell.getApiPort();
  current.value = info.port;
  range.value = info.range;
  input.value = String(info.port);
});

const dirty = computed(() => current.value !== null && input.value.trim() !== String(current.value));

/** 填到自动发现段之外：插件将无法自动找到本机 hawk，需要用户去插件高级选项里指定端口 */
const outsideRange = computed(() => {
  const port = Number(input.value.trim());
  return Number.isInteger(port) && (port < range.value[0] || port > range.value[1]);
});

async function apply() {
  const port = Number(input.value.trim());
  if (!Number.isInteger(port) || port < 1024 || port > 65535) {
    error.value = '端口须为 1024–65535 之间的整数';
    return;
  }
  saving.value = true;
  error.value = '';
  notice.value = '';
  try {
    const rejected = await shell.setApiPort(port);
    if (rejected) {
      error.value = rejected;
      return;
    }
    current.value = port;
    notice.value = '正在用新端口重启本地服务…';
    await shell.restartServer();
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="pane embedded">
    <div class="field column">
      <span class="field-label">本地 API 端口</span>
      <div class="port-row">
        <input v-model="input" type="text" inputmode="numeric" :disabled="saving" aria-label="本地 API 端口" @keydown.enter="apply()" />
        <button :disabled="saving || !dirty" @click="apply()">{{ saving ? '正在重启…' : '应用并重启' }}</button>
      </div>
      <p class="hint">
        hawk 桌面端本地服务的端口（被占用时启动屏会提示换端口，不会静默改端口）。浏览器插件按
        {{ range[0] }}–{{ range[1] }} 自动寻找本机
        hawk；填到这段之外时，需要在插件的高级选项里指定端口。正在远程浏览别的设备时改端口会重启本地服务并断开当前连接。
      </p>
      <p v-if="outsideRange" class="field-error">端口 {{ input.trim() }} 在自动发现区间 {{ range[0] }}–{{ range[1] }} 之外，浏览器插件将无法自动找到它。</p>
      <p v-if="error" class="field-error">{{ error }}</p>
      <p v-if="notice" class="hint">{{ notice }}</p>
    </div>
  </div>
</template>

<style scoped>
.port-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.port-row input {
  width: 120px;
  text-align: center;
}
</style>

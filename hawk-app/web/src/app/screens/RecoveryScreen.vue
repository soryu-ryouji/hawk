<script setup lang="ts">
// 局域网断线恢复等待：主进程每 10s 探测目标；恢复后自动重新 boot。
// 这里只提供显式的“打开本地素材库”和退出入口，不自动回切。
import { ref } from 'vue';
import { hasShell, shell } from '@/shared/lib/platform';
import { ipcMessage } from '@/shared/connection';

const props = defineProps<{ label: string }>();
const emit = defineEmits<{ local: []; quit: [] }>();

const busy = ref(false);
const error = ref('');

async function openLocal(): Promise<void> {
  if (!hasShell || busy.value) {
    return;
  }
  busy.value = true;
  error.value = '';
  try {
    // 主进程清掉局域网目标并切回本地；若没有本地库，返回 null，交给 App 进引导页。
    const conn = await shell.disconnectConnection();
    if (!conn) {
      emit('local');
    }
  } catch (e) {
    error.value = ipcMessage(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="recovery">
    <img class="logo" :src="'./icon.png'" alt="hawk" />
    <div class="title">与局域网素材库的连接已断开</div>
    <div class="target">{{ props.label }}</div>
    <p class="hint">每 10 秒自动重试一次，连接恢复后会自动回到素材库。</p>
    <div class="actions">
      <button class="primary" :disabled="busy" @click="openLocal">{{ busy ? '正在切换…' : '打开本地素材库' }}</button>
      <button :disabled="busy" @click="emit('quit')">退出 hawk</button>
    </div>
    <p v-if="error" class="error">{{ error }}</p>
  </div>
</template>

<style scoped>
.recovery {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 14px;
  padding: 0 24px 40px;
}

.logo {
  width: 96px;
  height: 96px;
  margin-bottom: 6px;
}

.title {
  font-size: 16px;
  font-weight: 600;
  text-align: center;
}

.target {
  max-width: 520px;
  color: var(--fg-1);
  font-size: 13px;
  text-align: center;
  word-break: break-all;
}

.hint {
  max-width: 520px;
  margin: 0;
  color: var(--fg-1);
  font-size: 12px;
  line-height: 1.6;
  text-align: center;
}

.actions {
  display: flex;
  gap: 8px;
  margin-top: 6px;
}

.actions button {
  padding: 8px 18px;
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

.primary:disabled {
  opacity: 0.5;
  cursor: default;
}

.error {
  margin: 0;
  max-width: 520px;
  color: var(--danger);
  font-size: 12px;
  text-align: center;
}
</style>

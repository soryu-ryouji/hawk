<script setup lang="ts">
// 目标端全局配对请求提示：轮询本机 daemon，收到请求后由用户选择只读/读写或拒绝。
import { onMounted, onUnmounted, ref } from 'vue';
import { hasShell, shell } from '@/shared/lib/platform';
import type { LanPairRequest } from '@/shared/types';

const current = ref<LanPairRequest | null>(null);
const busy = ref(false);
const error = ref('');

let timer = 0;

async function poll(): Promise<void> {
  if (!hasShell) {
    current.value = null;
    return;
  }
  try {
    const { requests } = await shell.lanPairRequests();
    current.value = requests[0] ?? null;
  } catch {
    // daemon 尚未就绪/无本地库时静默；下一轮继续。
  }
}

onMounted(() => {
  void poll();
  timer = window.setInterval(() => void poll(), 1800);
});
onUnmounted(() => window.clearInterval(timer));

async function decide(allow: boolean, write: boolean): Promise<void> {
  if (!current.value || busy.value) {
    return;
  }
  busy.value = true;
  error.value = '';
  try {
    await shell.lanDecidePairRequest(current.value.id, { allow, write });
    current.value = null;
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <Teleport to="body">
    <div v-if="current" class="pair-request-mask">
      <div class="pair-request-dialog" role="dialog" aria-modal="true" aria-labelledby="pair-request-title">
        <div id="pair-request-title" class="pair-request-title">局域网配对请求</div>
        <p class="pair-request-copy">
          <strong>{{ current.client_name }}</strong>
          请求连接此素材库
        </p>
        <p v-if="error" class="pair-request-error">{{ error }}</p>
        <div class="pair-request-actions">
          <button :disabled="busy" @click="decide(false, false)">拒绝</button>
          <button :disabled="busy" @click="decide(true, false)">仅浏览</button>
          <button class="primary" :disabled="busy" @click="decide(true, true)">允许写入</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.pair-request-mask {
  position: fixed;
  inset: 0;
  z-index: 240;
  display: grid;
  place-items: center;
  padding: 20px;
  background: rgba(0, 0, 0, 0.56);
}

.pair-request-dialog {
  width: min(360px, 100%);
  padding: 18px;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--bg-1);
  box-shadow: 0 18px 50px rgba(0, 0, 0, 0.45);
}

.pair-request-title {
  margin-bottom: 8px;
  font-size: 15px;
  font-weight: 600;
}

.pair-request-copy {
  margin: 0;
  color: var(--fg-0);
  font-size: 13px;
  line-height: 1.5;
}

.pair-request-error {
  margin: 10px 0 0;
  color: var(--danger);
  font-size: 12px;
}

.pair-request-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 18px;
}

.pair-request-actions button {
  padding: 5px 11px;
  font-size: 12px;
}

.primary {
  border-color: var(--accent);
  background: var(--accent);
  color: #fff;
}
</style>

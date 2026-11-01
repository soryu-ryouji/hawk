<script setup lang="ts">
// 可选视觉扩展：用户主动下载、启停或删除；未安装时不影响基础相同图片检索。
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { api } from '@/shared/api/endpoints';
import { errorText } from '@/shared/lib/storeUtil';
import { formatSize } from '@/shared/lib/format';
import type { VisionStatus } from '@/shared/types';

const status = ref<VisionStatus | null>(null);
const busy = ref(false);
const error = ref('');
let pollTimer: ReturnType<typeof setTimeout> | undefined;

const progress = computed(() => {
  const current = status.value;
  if (!current?.downloading || current.total_bytes === 0) {
    return 0;
  }
  return Math.min(100, Math.round((current.downloaded_bytes / current.total_bytes) * 100));
});

onMounted(async () => {
  await refresh();
});
onUnmounted(() => clearTimeout(pollTimer));

async function refresh() {
  try {
    status.value = await api.visionStatus();
    error.value = '';
    if (status.value.downloading || status.value.indexing || status.value.pending_items > 0) {
      pollTimer = setTimeout(() => void refresh(), 700);
    }
  } catch (e) {
    error.value = errorText(e);
  }
}

async function run(action: () => Promise<VisionStatus>) {
  busy.value = true;
  error.value = '';
  try {
    status.value = await action();
    pollTimer = setTimeout(() => void refresh(), 500);
  } catch (e) {
    error.value = errorText(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="pane">
    <section class="settings-section">
      <h3 class="settings-section-title">视觉扩展</h3>
      <div class="settings-group extension-card">
        <div class="extension-head">
          <div>
            <strong>相似构图</strong>
            <p>使用本地 DINOv2 模型分析配色、构图、风格和主体内容。图片不会离开此设备。</p>
          </div>
          <span class="extension-size">约 {{ formatSize(24446700) }}</span>
        </div>

        <div v-if="!status?.installed" class="extension-actions">
          <button class="primary" :disabled="busy || status?.downloading" @click="run(api.visionInstall)">
            {{ status?.downloading ? `正在下载 ${progress}%` : '下载并启用' }}
          </button>
        </div>
        <template v-else>
          <div class="switch-row">
            <div class="switch-label">启用视觉索引</div>
            <label class="switch">
              <input
                type="checkbox"
                :checked="status.enabled"
                :disabled="busy"
                @change="run(() => api.visionEnable(($event.target as HTMLInputElement).checked))"
              />
              <span class="track" />
            </label>
          </div>
          <p class="hint">
            已索引 {{ status.indexed_items }} / {{ status.total_items }} 项<template v-if="status.indexing && status.pending_items === 0"
              >，正在准备索引…</template
            ><template v-else-if="status.pending_items > 0">，等待分析 {{ status.pending_items }} 项</template>
          </p>
          <div class="extension-actions">
            <button :disabled="busy || status.downloading" @click="run(api.visionRemove)">删除模型</button>
          </div>
        </template>
        <p v-if="status?.downloading" class="hint">
          已下载 {{ formatSize(status.downloaded_bytes) }} / {{ formatSize(status.total_bytes) }}（{{ progress }}%）
        </p>
        <p v-if="status?.error || error" class="field-error">{{ error || status?.error }}</p>
      </div>
    </section>
  </div>
</template>

<style src="./settings-shared.css"></style>

<style scoped>
.extension-card {
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding: 14px;
}

.extension-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
}

.extension-head p {
  margin-top: 6px;
  color: var(--fg-1);
  font-size: 12px;
  line-height: 1.6;
}

.extension-size {
  flex: none;
  color: var(--fg-1);
  font-size: 12px;
}

.extension-actions {
  display: flex;
  justify-content: flex-end;
}

.primary {
  border-color: var(--accent);
  background: var(--accent);
  color: #fff;
}
</style>

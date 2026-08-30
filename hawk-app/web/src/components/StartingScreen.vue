<script setup lang="ts">
// 应用内启动屏：server 扫描索引期间的进度反馈（端口自旧独立 loading.html 页——
// 单页生命周期无二次导航，启动过程不会出现空白窗口；窗口在首帧渲染后才 show）。
// Electron 进度经主进程 IPC 推送；浏览器（局域网查看）由 useStartup 轮询驱动。
import WindowControls from './WindowControls.vue';
import type { ServerProgress } from '../composables/useStartup';

defineProps<{ progress: ServerProgress | null; error: string | null }>();
const emit = defineEmits<{ quit: [] }>();

const hasShell = !!window.hawkShell;
const isMac = window.hawkShell?.platform === 'darwin';

const LABELS: Record<string, string> = {
  scan: '正在扫描素材库…',
  hash: '正在计算文件哈希…',
  apply: '正在更新索引…',
  done: '即将完成…',
};
</script>

<template>
  <div class="starting">
    <!-- 自绘标题栏拖拽区（macOS 由系统红绿灯占据，不需要） -->
    <div v-if="!isMac" class="drag-bar" />
    <main>
      <!-- 动态绑定避免 vite 按模块解析；icon.png 来自 publicDir（build/） -->
      <img class="logo" :src="'./icon.png'" alt="hawk" />
      <template v-if="!error">
        <div class="status">{{ (progress && LABELS[progress.phase]) || '正在启动…' }}</div>
        <div class="bar">
          <div
            class="fill"
            :class="{ indeterminate: !progress || progress.total <= 0 }"
            :style="progress && progress.total > 0 ? { width: `${Math.min(100, Math.round((progress.processed / progress.total) * 100))}%` } : undefined"
          />
        </div>
        <div class="detail">
          {{ progress ? (progress.total > 0 ? `${progress.processed} / ${progress.total}` : progress.processed > 0 ? `已发现 ${progress.processed} 个文件` : '') : '' }}
        </div>
      </template>
      <template v-else>
        <div class="error-text">{{ error }}</div>
        <button v-if="hasShell" class="quit danger" @click="emit('quit')">退出 hawk</button>
      </template>
    </main>
    <WindowControls />
  </div>
</template>

<style scoped>
.starting {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.drag-bar {
  flex: none;
  height: 40px;
  -webkit-app-region: drag;
}

main {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 16px;
  padding-bottom: 40px; /* 视觉重心略上移，抵消 titlebar */
}

.logo {
  width: 96px;
  height: 96px;
}

.status {
  font-size: 15px;
}

.bar {
  width: 320px;
  height: 4px;
  border-radius: 2px;
  background: var(--border);
  overflow: hidden;
}

.fill {
  height: 100%;
  width: 0;
  border-radius: 2px;
  background: var(--accent);
  transition: width 0.15s ease-out;
}

/* 不定态：已知文件数但未知总数（遍历阶段）时横向扫动 */
.fill.indeterminate {
  width: 30%;
  animation: slide 1.1s ease-in-out infinite;
}

@keyframes slide {
  0% {
    margin-left: 0;
  }
  50% {
    margin-left: 70%;
  }
  100% {
    margin-left: 0;
  }
}

.detail {
  color: var(--fg-1);
  font-variant-numeric: tabular-nums;
  min-height: 20px;
}

.error-text {
  max-width: 480px;
  color: var(--danger);
  font-size: 14px;
  text-align: center;
  white-space: pre-wrap;
  user-select: text;
}

.quit {
  padding: 8px 24px;
  font-size: 14px;
}
</style>

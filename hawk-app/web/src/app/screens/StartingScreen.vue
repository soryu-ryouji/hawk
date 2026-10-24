<script setup lang="ts">
// 应用内启动屏：server 扫描索引期间的进度反馈（端口自旧独立 loading.html 页——
// 单页生命周期无二次导航，启动过程不会出现空白窗口；窗口在首帧渲染后才 show）。
// Electron 进度经主进程 IPC 推送；浏览器（局域网查看）由 useStartup 轮询驱动。
// 错误态两个分支：端口被占用（Port 冲突：自动/手输重试）与其余启动失败（只能退出）。
import { computed, onMounted, ref } from 'vue';
import WindowControls from '../chrome/WindowControls.vue';
import { hasShell, isMac, shell } from '@/shared/lib/platform';
import { PORT_IN_USE } from '@/shared/types';
import type { ServerProgress, ServerFailure } from '../startup';

const props = defineProps<{ progress: ServerProgress | null; error: string | null; failure: ServerFailure | null }>();
const emit = defineEmits<{ quit: [] }>();

/** 端口冲突（daemon 严格绑定失败）：给「自动挑可用端口」与「手输端口」两条出路。
 *  daemon 不再静默换端口，否则浏览器插件等生态客户端追不上（见 app-config.ts 的 API_PORT_RANGE） */
const portConflict = computed(() => (props.failure?.code === PORT_IN_USE ? props.failure : null));
const portInput = ref('');
/** 自动选择的候选段（主进程会告诉真实值，此处初始值与 app-config.ts 一致，仅避免首帧空文本） */
const range = ref<readonly [number, number]>([27371, 27400]);
const busy = ref(false);
const actionError = ref('');

onMounted(async () => {
  if (!hasShell) {
    return;
  }
  const info = await shell.getApiPort();
  range.value = info.range;
  portInput.value = String(info.port);
});

/** 自动：在候选段内挑第一个空闲端口，写入配置后重启 daemon */
async function retryAuto() {
  busy.value = true;
  actionError.value = '';
  try {
    const error = await shell.retryServerPort({ mode: 'auto' });
    if (error) {
      actionError.value = error;
    }
  } finally {
    busy.value = false;
  }
}

/** 手输：任意 1024–65535（主进程做最终校验与占用预检） */
async function retryManual() {
  const port = Number(portInput.value.trim());
  if (!Number.isInteger(port) || port < 1024 || port > 65535) {
    actionError.value = '端口须为 1024–65535 之间的整数';
    return;
  }
  busy.value = true;
  actionError.value = '';
  try {
    const error = await shell.retryServerPort({ port });
    if (error) {
      actionError.value = error;
    }
  } finally {
    busy.value = false;
  }
}

const LABELS: Record<string, string> = {
  scan: '正在扫描素材库…',
  hash: '正在计算文件哈希…',
  apply: '正在更新索引…',
  sync: '正在同步素材信息…',
  migrate: '正在迁移缓存…',
  // 主进程在 daemon 就绪后恢复上次浏览的远端库（server.ts 的 notifyReady）：走 launch 屏而非空页
  'restore-remote': '正在恢复远程素材库…',
  'restore-lan': '正在连接局域网素材库…',
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
          {{
            progress
              ? progress.total > 0
                ? `${progress.processed} / ${progress.total}`
                : progress.processed > 0
                  ? `已发现 ${progress.processed} 个文件`
                  : ''
              : ''
          }}
        </div>
      </template>
      <template v-else>
        <div class="error-text">{{ error }}</div>
        <!-- 端口被占用：不静默换端口，给出自动/手输两条出路（手输允许任意 1024–65535） -->
        <div v-if="portConflict" class="port-fix">
          <button class="primary" :disabled="busy" @click="retryAuto()">自动选择可用端口（{{ range[0] }}–{{ range[1] }}）</button>
          <div class="port-manual">
            <input v-model="portInput" type="text" inputmode="numeric" :disabled="busy" aria-label="手动指定端口" @keydown.enter="retryManual()" />
            <button :disabled="busy" @click="retryManual()">用该端口重试</button>
          </div>
          <div class="hint">
            端口被其他程序占用（或另一个 hawk 实例还在运行）。自动选择只在 {{ range[0] }}–{{ range[1] }} 段内挑， 浏览器插件按这一段自动寻找本机
            hawk；填到段外时需在插件高级选项里指定端口。
          </div>
          <div v-if="actionError" class="error-text">{{ actionError }}</div>
        </div>
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

/* ---- 端口冲突处理区 ---- */
.port-fix {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  max-width: 480px;
}

.port-manual {
  display: flex;
  gap: 8px;
}

.port-manual input {
  width: 110px;
  text-align: center;
}

.port-fix .hint {
  font-size: 12px;
  line-height: 1.6;
  color: var(--fg-1);
  text-align: center;
  margin: 0;
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}
</style>

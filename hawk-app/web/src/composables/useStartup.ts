// 启动阶段状态机：连接就绪前应用只显示启动屏（StartingScreen.vue）。
// Electron 由主进程推送（hawk:server-started / hawk:server-error / hawk:server-progress）；
// 纯浏览器（局域网查看）无 IPC，自行轮询 /app/startup 直至 ready/error（401 转 ConnectScreen）。
import { onMounted, onUnmounted, ref } from 'vue';
import { api } from '../api/endpoints';
import { ApiError, configureApi } from '../api/client';

export interface ServerProgress {
  phase: string;
  processed: number;
  total: number;
}

export function useStartup() {
  /** 每次 server 就绪 +1（冷启动、换库、应用设置重启都会触发）；App 据它 (re)boot */
  const readyCount = ref(0);
  /** 启动/运行失败原因；'UNAUTHORIZED' 为哨兵值（浏览器 token 失效 → ConnectScreen） */
  const failed = ref<string | null>(null);
  const progress = ref<ServerProgress | null>(null);

  let pollTimer = 0;
  let polling = false;

  function stopPolling() {
    clearTimeout(pollTimer);
    polling = false;
  }

  /** 浏览器路径：轮询启动状态。Electron 路径（有 hawkShell）为空操作，就绪走 IPC */
  async function poll(): Promise<void> {
    if (polling || window.hawkShell) {
      return;
    }
    polling = true;
    while (polling) {
      try {
        const s = await api.startupStatus();
        if (s.status === 'ready') {
          stopPolling();
          readyCount.value++;
          return;
        }
        if (s.status === 'error') {
          stopPolling();
          failed.value = s.message ?? '初始索引构建失败';
          return;
        }
        progress.value = {
          phase: s.phase ?? 'scan',
          processed: Number(s.processed ?? 0),
          total: Number(s.total ?? 0),
        };
      } catch (e) {
        if (e instanceof ApiError && e.httpStatus === 401) {
          stopPolling();
          failed.value = 'UNAUTHORIZED';
          return;
        }
        // NETWORK 等：server 未监听，继续轮询
      }
      await new Promise((resolve) => {
        pollTimer = window.setTimeout(resolve, 400);
      });
    }
  }

  onMounted(() => {
    const shell = window.hawkShell;
    if (!shell) {
      return;
    }
    shell.onServerProgress((p) => {
      progress.value = p;
    });
    shell.onServerError(({ message }) => {
      failed.value = message;
    });
    // server 重启会换新地址/token（轮询端口即变）：先重配连接再计就绪
    shell.onServerStarted((conn) => {
      configureApi({ api: conn.address, token: conn.token });
      readyCount.value++;
    });
  });
  onUnmounted(() => stopPolling());

  return { readyCount, failed, progress, poll };
}

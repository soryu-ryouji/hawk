// 启动阶段状态机：连接就绪前应用只显示启动屏（StartingScreen.vue）。
// Electron 由主进程推送（hawk:server-started / hawk:server-error / hawk:server-progress）；
// 纯浏览器（局域网查看）无 IPC，自行轮询 /app/startup 直至 ready/error（401 转 ConnectScreen）。
import { onMounted, onUnmounted, ref } from 'vue';
import { api } from '@/shared/api/endpoints';
import { ApiError, configureApi } from '@/shared/api/client';
import { hasShell, shell } from '@/shared/lib/platform';
import type { ServerFailure } from '@/shared/types';

export interface ServerProgress {
  phase: string;
  processed: number;
  total: number;
}

export type { ServerFailure };

export function useStartup() {
  /** 每次 server 就绪 +1（冷启动、换库、应用设置重启都会触发）；App 据它 (re)boot */
  const readyCount = ref(0);
  /** 启动/运行失败原因；'UNAUTHORIZED' 为哨兵值（浏览器 token 失效 → ConnectScreen） */
  const failed = ref<string | null>(null);
  /** 失败详情（Electron）：带 code 时可分流（PORT_IN_USE → 启动屏端口冲突界面） */
  const failure = ref<ServerFailure | null>(null);
  const progress = ref<ServerProgress | null>(null);

  let pollTimer = 0;
  let polling = false;

  function stopPolling() {
    clearTimeout(pollTimer);
    polling = false;
  }

  /** 浏览器路径：轮询启动状态。Electron 路径（有 hawkShell）为空操作，就绪走 IPC */
  async function poll(): Promise<void> {
    if (polling || hasShell) {
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
          failure.value = { message: failed.value };
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
    if (!hasShell) {
      return;
    }
    shell.onServerProgress((p) => {
      progress.value = p;
      // 新一轮启动在推进：清掉上一轮的失败详情（含端口冲突态），否则重试后旧错误会残留
      failure.value = null;
    });
    shell.onServerError((e) => {
      failure.value = e;
      failed.value = e.message;
    });
    // server 重启会换新地址/token（轮询端口即变）：先重配连接再计就绪。
    // 事件与拉取双通道去重：onServerStarted 只发一次，页面（重）加载晚于就绪时会丢，
    // 故挂载时同时经 remoteGetConnection 拉取——同连接只 boot 一次。
    // 远程切换（connection-changed）与断线回切走同一条重配路径：连接描述符即「一个 api base + 一个 token」
    let bootedKey: string | null = null;
    const onConn = (conn: { address?: string; api_base?: string; token: string }) => {
      const api = conn.api_base ?? conn.address;
      if (!api) {
        return;
      }
      const key = `${api}|${conn.token}`;
      if (bootedKey === key) {
        return;
      }
      bootedKey = key;
      configureApi({ api, token: conn.token });
      readyCount.value++;
    };
    shell.onServerStarted(onConn);
    shell.onConnectionChanged(onConn);
    // 故障拉取兜底：daemon 端口占用会在页面订阅 IPC 之前就退出，只靠 onServerError 推送会丢事件
    void shell.getServerError().then((e) => {
      if (e) {
        failure.value = e;
        failed.value = e.message;
      }
    });
    void shell.remoteGetConnection().then((conn) => {
      if (conn) {
        onConn(conn);
      }
    });
  });
  onUnmounted(() => stopPolling());

  return { readyCount, failed, failure, progress, poll };
}

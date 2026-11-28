// 启动相位机与 SSE 桥（装配层）：phase 状态机、runBoot 编排（拉数据 + 连 SSE）、
// server 重启监听、浏览器路径轮询接线。从 App.vue 抽出，App 只做相位分发与模板。
// 相位：starting（等 server 就绪）→ ready（主界面）；旁路 setup（未配置库）/ connect（token 门页）/ error。
import { onMounted, onUnmounted, ref, watch } from 'vue';
import { initApi, apiConfig, clearStoredCredential, ApiError } from '@/shared/api/client';
import { api } from '@/shared/api/endpoints';
import { connectEvents } from '@/shared/api/events';
import { hasShell, shell } from '@/shared/lib/platform';
import { useStartup, type ServerFailure } from './startup';
import { useLibraryStore } from '@/domains/library';
import { useTaxonomyStore } from '@/domains/taxonomy';
import { usePreviewStore } from '@/domains/preview';
import { useLanSettingsStore } from '@/domains/settings';

export type BootPhase = 'starting' | 'ready' | 'setup' | 'connect' | 'error' | 'reconnecting';

export function useBoot() {
  const phase = ref<BootPhase>('starting');
  const bootError = ref<string | null>(null);
  /** 失败详情（Electron：code=PORT_IN_USE 时启动屏显示端口冲突处理），浏览器路径为 null */
  const bootFailure = ref<ServerFailure | null>(null);
  /** 恢复等待界面展示的目标名 */
  const recoveryLabel = ref('局域网素材库');
  const { readyCount, failed, failure, progress, poll } = useStartup();
  let disconnectEvents: (() => void) | null = null;
  /** 当前是否因局域网断开进入恢复等待；成功 boot 后清除 */
  let recovering = false;
  /** boot 代次：断线/切换连接时作废旧的在途 boot，避免旧结果覆盖恢复相位 */
  let bootSeq = 0;
  /** 恢复等待中重新 boot 失败时的 10s 重试定时器 */
  let recoveryRetryTimer: ReturnType<typeof setTimeout> | null = null;

  function clearRecoveryRetry(): void {
    if (recoveryRetryTimer) {
      clearTimeout(recoveryRetryTimer);
      recoveryRetryTimer = null;
    }
  }

  function scheduleRecoveryRetry(): void {
    clearRecoveryRetry();
    recoveryRetryTimer = setTimeout(() => {
      if (phase.value === 'reconnecting' && recovering) {
        phase.value = 'starting';
        void runBoot();
      }
    }, 10_000);
  }

  // 连接就绪（冷启动/换库/应用设置重启都会触发）：拉取数据 + 重连 SSE，成功后进入主界面。
  // server 重启会换新地址，useStartup 已先重配 API；SSE 先断后连，避免挂到旧服务上
  async function runBoot() {
    const seq = ++bootSeq;
    bootError.value = null;
    const store = useLibraryStore();
    const taxonomy = useTaxonomyStore();
    const preview = usePreviewStore();
    const lanSettings = useLanSettingsStore();
    try {
      // 换库/断开远程/应用设置重启复用本入口：先清上一库的预览/编辑浮层会话状态（跨 store 会话
      // 清理由装配层编排，store 之间不互相调用初始化逻辑），再按连接形态分流
      preview.closePreview();
      preview.closeEditor();
      // 浏览器先以 AccessKey 换 Session Cookie，后续图片/SSE 不再通过 URL 传 token。
      if (!hasShell) {
        await api.appSession(apiConfig().credential);
      }
      const info = await api.appInfo();
      if (seq !== bootSeq) {
        return;
      }
      // 分类维度先行：主 store init 的 restoreView 校验依赖 taxonomy 数据（validators 注入，保持 store 间 DAG）
      await taxonomy.refreshAll();
      if (seq !== bootSeq) {
        return;
      }
      await store.init(
        {
          folderExists: taxonomy.folderExists,
          categoryExists: taxonomy.categoryExists,
          tagExists: taxonomy.tagExists,
        },
        info,
      );
      if (seq !== bootSeq) {
        return;
      }
      disconnectEvents?.();
      const events = connectEvents({
        onAdded: (item) => store.applyEvent('item.added', item),
        onItemsAdded: () => store.applyEvent('items.added', null),
        onUpdated: (item) => store.applyEvent('item.updated', item),
        onItemsUpdated: (items) => store.applyEvent('items.updated', items),
        onTrashed: (id) => store.applyEvent('item.trashed', { id }),
        onRestored: (item) => store.applyEvent('item.restored', item),
        onRemoved: (id) => store.applyEvent('item.removed', { id }),
        onTaskProgress: (p) => store.applyEvent('task.progress', p),
        onFolderChanged: () => store.applyEvent('folder.changed', {}),
        onLibraryUpdated: (info) => store.applyEvent('library.updated', info),
        onGlobalFilterChanged: (filter) => store.applyEvent('global_filter.changed', filter),
        onLocksChanged: (locks) => store.applyEvent('locks.changed', locks),
        onLanAccessChanged: () => {
          void store.refreshAccess();
          // AccessKey 设置页由本机 admin 读取。
          if (hasShell && store.role === 'admin') {
            void lanSettings.load(true);
          }
        },
        onReconnect: () => {
          void store.reloadSkeleton();
          void taxonomy.refreshFolders();
          void taxonomy.refreshGlobalFilter();
        },
      });
      disconnectEvents = events;
      if (seq !== bootSeq) {
        if (disconnectEvents === events) {
          disconnectEvents = null;
        }
        events();
        return;
      }
      clearRecoveryRetry();
      recovering = false;
      phase.value = 'ready';
    } catch (e) {
      if (seq !== bootSeq) {
        return;
      }
      // 恢复等待中网络仍不可达：留在恢复界面，主进程和这里的 10s 定时器继续重试。
      if (recovering && e instanceof ApiError && e.code === 'NETWORK') {
        phase.value = 'reconnecting';
        scheduleRecoveryRetry();
        return;
      }
      if (e instanceof ApiError && e.code === 'UNAUTHORIZED') {
        // token 缺失/失效：清掉本地残留，进门页重新输入
        clearStoredCredential(apiConfig().api);
        phase.value = 'connect';
      } else {
        bootError.value = e instanceof Error ? e.message : String(e);
        phase.value = 'error';
      }
    }
  }

  watch(readyCount, () => {
    // 冷启动已在 starting；换库/应用设置重启时 phase 还是 ready（主界面挂着旧数据）：
    // 先回启动屏（进度经 IPC 持续到达）再重启数据，避免换库期间主界面假死误导
    clearRecoveryRetry();
    recovering = false;
    phase.value = 'starting';
    void runBoot();
  });

  // server 启动/运行失败：token 哨兵值转门页，其余进错误屏
  watch(failed, (message) => {
    if (!message) {
      return;
    }
    if (message === 'UNAUTHORIZED') {
      clearStoredCredential(apiConfig().api);
      phase.value = 'connect';
    } else {
      bootError.value = message;
      bootFailure.value = failure.value;
      phase.value = 'error';
    }
  });

  /** 主进程判定局域网断开：保留目标，进入恢复等待；在途 boot 作废，SSE 先停。 */
  function markReconnecting(notice: { label?: string; peer_id?: string }, force = false): void {
    if (!force && (phase.value === 'setup' || phase.value === 'connect')) {
      return;
    }
    clearRecoveryRetry();
    recovering = true;
    bootSeq += 1;
    recoveryLabel.value = notice.label || '局域网素材库';
    disconnectEvents?.();
    disconnectEvents = null;
    phase.value = 'reconnecting';
  }

  /** 主进程探测到目标恢复：重新走完整 boot（重新拉数据 + 重连 SSE）。 */
  function handleConnectionRestored(notice: { label?: string; peer_id?: string }): void {
    if (phase.value !== 'reconnecting') {
      return;
    }
    clearRecoveryRetry();
    recoveryLabel.value = notice.label || recoveryLabel.value;
    progress.value = { phase: 'restore-lan', processed: 0, total: 0 };
    phase.value = 'starting';
    void runBoot();
  }

  // 初始相位判定（同步，须在上方 immediate 监听器注册前完成）：无连接参数时把 starting
  // 纠正为 setup（Electron 进引导）/ error（浏览器），否则 immediate 监听器会对 setup/error
  // 误发起轮询。注：App.vue 原实现里这段在 watch 之后（与注释声明相反，浏览器无参数时
  // 会先启动注定失败的轮询），抽取时按注释意图修正顺序
  if (!initApi()) {
    phase.value = hasShell ? 'setup' : 'error';
    bootError.value = hasShell ? null : '缺少后端连接参数';
  }

  // 浏览器路径（无 IPC）：进入 starting 即自行轮询启动状态。
  // immediate 必须——移动端（局域网浏览器）初始即 starting 且不再变化，不立即触发会永远卡在启动屏
  watch(
    phase,
    (p) => {
      if (p === 'starting' && !hasShell) {
        void poll();
      }
    },
    { immediate: true },
  );

  let offConnectionLost: (() => void) | null = null;
  let offConnectionRestored: (() => void) | null = null;

  onMounted(() => {
    if (!hasShell) {
      return;
    }
    // 换库/应用设置重启：主进程停旧 server 时即收到事件（早于 ready），
    // 立刻切启动屏——旧 server 已停、新 server 未 ready 的窗口期主界面 API 全失效（假死）
    shell.onServerRestarting(() => {
      if (phase.value !== 'reconnecting') {
        phase.value = 'starting';
      }
    });
    offConnectionLost = shell.onConnectionLost((notice) => markReconnecting(notice));
    offConnectionRestored = shell.onConnectionRestored((notice) => handleConnectionRestored(notice));
    // 页面加载晚于断线事件：主动拉一次状态，直接在恢复等待界面启动。
    void shell
      .getConnectionState()
      .then((state) => {
        if (state.reconnecting) {
          markReconnecting({ label: state.label ?? undefined }, true);
        }
      })
      .catch(() => {});
  });

  onUnmounted(() => {
    clearRecoveryRetry();
    disconnectEvents?.();
    offConnectionLost?.();
    offConnectionRestored?.();
  });

  return { phase, bootError, bootFailure, progress, recoveryLabel, runBoot };
}

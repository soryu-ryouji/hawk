// 启动相位机与 SSE 桥（装配层）：phase 状态机、runBoot 编排（拉数据 + 连 SSE）、
// server 重启监听、浏览器路径轮询接线。从 App.vue 抽出，App 只做相位分发与模板。
// 相位：starting（等 server 就绪）→ ready（主界面）；旁路 setup（未配置库）/ connect（token 门页）/ error。
import { onMounted, onUnmounted, ref, watch } from 'vue';
import { initApi, apiConfig, clearStoredToken, ApiError } from '@/shared/api/client';
import { connectEvents } from '@/shared/api/events';
import { hasShell, shell } from '@/shared/lib/platform';
import { useStartup } from './startup';
import { useLibraryStore } from '@/domains/library';
import { useTaxonomyStore } from '@/stores/taxonomy';
import { usePreviewStore } from '@/stores/preview';

export type BootPhase = 'starting' | 'ready' | 'setup' | 'connect' | 'error';

export function useBoot() {
  const phase = ref<BootPhase>('starting');
  const bootError = ref<string | null>(null);
  const { readyCount, failed, progress, poll } = useStartup();
  let disconnectEvents: (() => void) | null = null;

  // 连接就绪（冷启动/换库/应用设置重启都会触发）：拉取数据 + 重连 SSE，成功后进入主界面。
  // server 重启会换新地址，useStartup 已先重配 API；SSE 先断后连，避免挂到旧服务上
  async function runBoot() {
    bootError.value = null;
    const store = useLibraryStore();
    const taxonomy = useTaxonomyStore();
    const preview = usePreviewStore();
    try {
      // 换库/应用设置重启复用本入口：先清上一库的预览/编辑浮层会话状态（跨 store 会话清理由
      // 装配层编排，store 之间不互相调用初始化逻辑），再重启数据
      preview.closePreview();
      preview.closeEditor();
      // 分类维度先行：主 store init 的 restoreView 校验依赖 taxonomy 数据（validators 注入，保持 store 间 DAG）
      await taxonomy.refreshAll();
      await store.init({
        folderExists: taxonomy.folderExists,
        categoryExists: taxonomy.categoryExists,
        tagExists: taxonomy.tagExists,
      });
      disconnectEvents?.();
      disconnectEvents = connectEvents({
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
        onReconnect: () => {
          void store.reloadSkeleton();
          void taxonomy.refreshFolders();
          void taxonomy.refreshGlobalFilter();
        },
      });
      phase.value = 'ready';
    } catch (e) {
      if (e instanceof ApiError && e.code === 'UNAUTHORIZED') {
        // token 缺失/失效：清掉本地残留，进门页重新输入
        clearStoredToken(apiConfig().api);
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
    phase.value = 'starting';
    void runBoot();
  });

  // server 启动/运行失败：token 哨兵值转门页，其余进错误屏
  watch(failed, (message) => {
    if (!message) {
      return;
    }
    if (message === 'UNAUTHORIZED') {
      clearStoredToken(apiConfig().api);
      phase.value = 'connect';
    } else {
      bootError.value = message;
      phase.value = 'error';
    }
  });

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

  onMounted(() => {
    // 换库/应用设置重启：主进程停旧 server 时即收到事件（早于 ready），
    // 立刻切启动屏——旧 server 已停、新 server 未 ready 的窗口期主界面 API 全失效（假死）
    shell.onServerRestarting(() => {
      phase.value = 'starting';
    });
  });

  onUnmounted(() => {
    disconnectEvents?.();
  });

  return { phase, bootError, progress, runBoot };
}

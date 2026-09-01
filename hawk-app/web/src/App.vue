<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { initApi, apiConfig, clearStoredToken, ApiError } from './api/client';
import { connectEvents } from './api/events';
import { useLibraryStore } from './stores/library';
import { useShortcuts } from './composables/useShortcuts';
import { useDragImport } from './composables/useDragImport';
import { useLayout } from './composables/useLayout';
import { useStartup } from './composables/useStartup';
import Sidebar from './components/Sidebar.vue';
import TitleBar from './components/TitleBar.vue';
import FilterBar from './components/FilterBar.vue';
import WindowControls from './components/WindowControls.vue';
import ItemGrid from './components/ItemGrid.vue';
import Inspector from './components/Inspector.vue';
import PreviewOverlay from './components/PreviewOverlay.vue';
import ImageEditDialog from './components/ImageEditDialog.vue';
import ContextMenu from './components/ContextMenu.vue';
import SetupScreen from './components/SetupScreen.vue';
import ConnectScreen from './components/ConnectScreen.vue';
import StartingScreen from './components/StartingScreen.vue';
import SettingsDialog from './components/SettingsDialog.vue';

const store = useLibraryStore();
const { narrow, touch } = useLayout();
// 启动阶段状态机：starting（应用内启动屏，等 server 就绪）→ ready（主界面）；
// 旁路：setup（未配置素材库）/ connect（浏览器 token 门页）/ error（启动失败）。
// 就绪信号与进度来自 useStartup（Electron 走 IPC，浏览器走轮询）
const phase = ref<'starting' | 'ready' | 'setup' | 'connect' | 'error'>('starting');
const bootError = ref<string | null>(null);
const { readyCount, failed, progress, poll } = useStartup();
// 窄屏下侧栏为抽屉式：进入窄屏默认收起（抽屉关闭），回宽屏恢复双栏
watch(
  narrow,
  (isNarrow) => {
    store.sidebarVisible = !isNarrow;
  },
  { immediate: true },
);
const showSettings = ref(false);
let disconnectEvents: (() => void) | null = null;

// 索引进度条文案：扫描中带阶段进度（遍历阶段总数未知，只报已处理数）；否则报剩余任务数
const indexProgressText = computed(() => {
  const p = store.indexProgress;
  if (!p) {
    return '';
  }
  if (p.phase) {
    const label = p.phase === 'scan' ? '扫描' : p.phase === 'hash' ? '哈希' : '应用';
    const total = p.total ?? 0;
    const processed = p.processed ?? 0;
    return total > 0 ? `正在索引素材库 · ${label} ${processed}/${total}` : `正在索引素材库 · 已发现 ${processed} 个文件`;
  }
  return `正在索引素材 · 剩余 ${p.pending + p.active}`;
});

// ---- 侧栏宽度拖拽 ----
const SIDEBAR_MIN = 180;
const SIDEBAR_MAX = 480;
const INSPECTOR_MIN = 240;
const INSPECTOR_MAX = 560;
const sidebarWidth = ref(220);
const inspectorWidth = ref(280);

function clamp(v: number, min: number, max: number) {
  return Math.min(max, Math.max(min, Math.round(v)));
}

function loadPanelWidths() {
  try {
    const saved = JSON.parse(localStorage.getItem('hawk:panelWidths') ?? '{}') as {
      sidebar?: number;
      inspector?: number;
    };
    if (typeof saved.sidebar === 'number') {
      sidebarWidth.value = clamp(saved.sidebar, SIDEBAR_MIN, SIDEBAR_MAX);
    }
    if (typeof saved.inspector === 'number') {
      inspectorWidth.value = clamp(saved.inspector, INSPECTOR_MIN, INSPECTOR_MAX);
    }
  } catch {
    // 损坏的持久化数据忽略
  }
}

function savePanelWidths() {
  localStorage.setItem(
    'hawk:panelWidths',
    JSON.stringify({ sidebar: sidebarWidth.value, inspector: inspectorWidth.value }),
  );
}

function onResizeMove(e: MouseEvent) {
  if (dragSide.value === 'left') {
    sidebarWidth.value = clamp(e.clientX, SIDEBAR_MIN, SIDEBAR_MAX);
  } else if (dragSide.value === 'right') {
    inspectorWidth.value = clamp(window.innerWidth - e.clientX, INSPECTOR_MIN, INSPECTOR_MAX);
  }
}

function stopResize() {
  dragSide.value = null;
  document.body.classList.remove('col-resizing');
  window.removeEventListener('mousemove', onResizeMove);
  window.removeEventListener('mouseup', stopResize);
  savePanelWidths();
}

const dragSide = ref<'left' | 'right' | null>(null);

function startResize(side: 'left' | 'right') {
  dragSide.value = side;
  document.body.classList.add('col-resizing');
  window.addEventListener('mousemove', onResizeMove);
  window.addEventListener('mouseup', stopResize);
}

// 连接就绪（冷启动/换库/应用设置重启都会触发）：拉取数据 + 重连 SSE，成功后进入主界面。
// server 重启会换新地址，useStartup 已先重配 API；SSE 先断后连，避免挂到旧服务上
async function runBoot() {
  bootError.value = null;
  try {
    await store.init();
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
      onReconnect: () => {
        void store.reloadSkeleton();
        void store.refreshFolders();
      },
    });
    phase.value = 'ready';
  } catch (e) {
    if (e instanceof ApiError && e.code === 'UNAUTHORIZED') {
      // token 缺失/失效：清掉本地残留,进门页重新输入
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

// server 启动/运行失败：token 哨兵值转门页，其余进错误屏（含退出入口）
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

// 浏览器路径（无 IPC）：进入 starting 即自行轮询启动状态。
// immediate 必须——移动端（局域网浏览器）初始即 starting 且不再变化，不立即触发会永远卡在启动屏
watch(
  phase,
  (p) => {
    if (p === 'starting' && !window.hawkShell) {
      void poll();
    }
  },
  { immediate: true },
);

// ConnectScreen 验证通过：token 已注入，重回 starting。浏览器靠轮询就绪后 runBoot；
// Electron 的 server 必已就绪（token 由主进程持有，401 场景实际不可达），直接 boot
function onConnected() {
  phase.value = 'starting';
  if (window.hawkShell) {
    void runBoot();
  }
}

function quitApp() {
  void window.hawkShell?.quitApp();
}

/** web 端注销 token：清除本浏览器记忆的局域网 token，回门页重新输入（切换只读/可写身份用） */
function logoutToken() {
  clearStoredToken(apiConfig().api);
  showSettings.value = false;
  phase.value = 'connect';
}

/** 触屏窄屏：点击侧栏导航项（智能条目/文件夹/分类/标签）后收起抽屉；鼠标设备保持展开（桌面窄窗可连续切换） */
function onSidebarNav(e: MouseEvent) {
  if (!narrow.value || !touch.value) {
    return;
  }
  if ((e.target as HTMLElement).closest('.entry, .node, .cat-row, .tag-row')) {
    store.toggleSidebar();
  }
}

// 初始阶段判定（同步，须在 phase 监听器注册前完成：无参数时把 starting 纠正为 setup/error，
// 否则 immediate 监听器会对 setup/error 误发起轮询）
if (!initApi()) {
  phase.value = window.hawkShell ? 'setup' : 'error';
  bootError.value = window.hawkShell ? null : '缺少后端连接参数';
}

onMounted(() => {
  loadPanelWidths();
  // 换库/应用设置重启：主进程停旧 server 时即收到事件（早于 ready），
  // 立刻切启动屏——旧 server 已停、新 server 未 ready 的窗口期主界面 API 全失效（假死）
  window.hawkShell?.onServerRestarting(() => {
    phase.value = 'starting';
  });
});

onUnmounted(() => {
  disconnectEvents?.();
  // 拖拽中组件被卸载（如切到引导页）时兜底清理
  if (dragSide.value) {
    stopResize();
  }
});

useShortcuts();
useDragImport();
</script>

<template>
  <!-- 启动/引导/门页/错误：单页内的前置阶段；无边框窗口下仍需拖拽区与窗口控制按钮 -->
  <div v-if="phase !== 'ready'" class="standalone">
    <div class="drag-bar"><WindowControls /></div>
    <StartingScreen
      v-if="phase === 'starting' || phase === 'error'"
      :progress="progress"
      :error="phase === 'error' ? bootError : null"
      @quit="quitApp"
    />
    <SetupScreen v-else-if="phase === 'setup'" @selected="phase = 'starting'" />
    <ConnectScreen v-else-if="phase === 'connect'" @connect="onConnected" />
  </div>

  <!-- Eagle 式布局：侧栏/检查器通高，标题栏只覆盖中栏；窗口控制 fixed 于窗口右上角（Windows/Linux） -->
  <div
    v-else
    class="app"
    :class="{ 'no-panels': !store.sidebarVisible, mobile: narrow, 'drawer-open': narrow && store.sidebarVisible }"
    :style="{
      gridTemplateColumns: narrow
        ? 'minmax(0, 1fr)'
        : store.sidebarVisible
          ? `${sidebarWidth}px minmax(0, 1fr) ${inspectorWidth}px`
          : '0 minmax(0, 1fr) 0',
    }"
  >
    <!-- display:contents 包裹层仅用于移动端「导航后收起抽屉」的点击委托，不改变网格布局 -->
    <div class="sidebar-wrap" @click="onSidebarNav">
      <Sidebar class="sidebar" />
    </div>
    <!-- 窄屏抽屉遮罩：点按空白处收起 -->
    <div v-if="narrow && store.sidebarVisible" class="drawer-scrim" @click="store.toggleSidebar()" />
    <TitleBar class="titlebar" @open-settings="showSettings = true" />
    <!-- 筛选工具列：顶栏漏斗按钮展开，或评分/颜色条件激活时常驻 -->
    <FilterBar v-if="store.filterBarVisible || store.hasActiveFilters" />
    <ItemGrid />
    <!-- 索引进度指示：入库队列/扫描进度（与缩略图条同为只读指示） -->
    <div v-if="store.indexProgress" class="task-bar index">
      <div class="task-bar-fill" />
      <span class="task-bar-text">{{ indexProgressText }}</span>
    </div>
    <!-- 缩略图后台积压指示：细进度条压在网格顶缘（浏览器式加载条），计数归零自动消失 -->
    <div v-if="store.taskBacklog" class="task-bar">
      <div class="task-bar-fill" />
      <span class="task-bar-text">正在生成缩略图 · 剩余 {{ store.taskBacklog.pending + store.taskBacklog.active }}</span>
    </div>
    <Inspector class="inspector" />
    <WindowControls />

    <!-- 侧栏宽度拖拽手柄：4px 命中区紧贴分界线右侧，避开左侧面板的滚动条 -->
    <template v-if="store.sidebarVisible && !narrow">
      <div
        class="col-resize-handle"
        :class="{ active: dragSide === 'left' }"
        :style="{ left: `${sidebarWidth}px` }"
        @mousedown.prevent="startResize('left')"
      />
      <div
        class="col-resize-handle"
        :class="{ active: dragSide === 'right' }"
        :style="{ left: `calc(100% - ${inspectorWidth}px)` }"
        @mousedown.prevent="startResize('right')"
      />
    </template>

    <PreviewOverlay
      v-if="store.previewItem"
      :item="store.previewItem"
      @close="store.closePreview()"
      @navigate="store.navigatePreview($event)"
    />
    <!-- 图片编辑窗口:网格/预览浮层右键「编辑图片…」打开,层级高于预览浮层 -->
    <ImageEditDialog v-if="store.editorTarget" :item="store.editorTarget" @close="store.closeEditor()" />
    <SettingsDialog v-if="showSettings" @close="showSettings = false" @logout="logoutToken" />
    <ContextMenu />

    <Teleport to="body">
      <!-- 导入进度：拖拽落下即显示（收集文件阶段为不定态），逐个处理完推进 -->
      <div v-if="store.importProgress" class="import-progress">
        <span class="import-progress-text">
          {{
            store.importProgress.total > 0
              ? `正在导入 ${store.importProgress.done} / ${store.importProgress.total}`
              : '正在收集文件…'
          }}
        </span>
        <div class="import-progress-track">
          <div
            class="import-progress-bar"
            :class="{ indeterminate: store.importProgress.total === 0 }"
            :style="
              store.importProgress.total > 0
                ? { width: `${(store.importProgress.done / store.importProgress.total) * 100}%` }
                : undefined
            "
          />
        </div>
      </div>
      <div v-if="store.toast" class="toast" :class="{ 'toast-raised': store.importProgress }">{{ store.toast }}</div>
    </Teleport>
  </div>
</template>

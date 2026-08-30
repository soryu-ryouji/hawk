<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue';
import { initApi } from './api/client';
import { connectEvents } from './api/events';
import { useLibraryStore } from './stores/library';
import { useShortcuts } from './composables/useShortcuts';
import { useDragImport } from './composables/useDragImport';
import Sidebar from './components/Sidebar.vue';
import TitleBar from './components/TitleBar.vue';
import WindowControls from './components/WindowControls.vue';
import ItemGrid from './components/ItemGrid.vue';
import Inspector from './components/Inspector.vue';
import PreviewOverlay from './components/PreviewOverlay.vue';
import ImageEditDialog from './components/ImageEditDialog.vue';
import ContextMenu from './components/ContextMenu.vue';
import SetupScreen from './components/SetupScreen.vue';

const store = useLibraryStore();
const bootError = ref<string | null>(null);
// 无连接参数但在 Electron 内：素材库未配置，进引导页
const setupMode = ref(false);
let disconnectEvents: (() => void) | null = null;

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

async function boot() {
  if (!initApi()) {
    if (window.hawkShell) {
      setupMode.value = true;
    } else {
      bootError.value = '缺少后端连接参数';
    }
    return;
  }
  try {
    await store.init();
  } catch {
    bootError.value = '无法连接 hawk-server，请确认后端已启动';
    return;
  }
  setupMode.value = false;
  bootError.value = null;
  disconnectEvents?.();
  disconnectEvents = connectEvents({
    onAdded: (item) => store.applyEvent('item.added', item),
    onUpdated: (item) => store.applyEvent('item.updated', item),
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
}

// 引导页选定素材库后，主进程仅在原 URL 上改 hash 注入连接参数；仅 hash 变化的导航是
// same-document 导航（页面不重载、onMounted 不重跑），需监听 hashchange 重新走启动流程，
// 否则会一直停留在引导页
function onHashChange() {
  if (setupMode.value || bootError.value) {
    void boot();
  }
}

onMounted(() => {
  loadPanelWidths();
  void boot();
  window.addEventListener('hashchange', onHashChange);
});

onUnmounted(() => {
  window.removeEventListener('hashchange', onHashChange);
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
  <!-- 引导页/启动失败页：无边框窗口下仍需拖拽区与窗口控制按钮 -->
  <div v-if="setupMode || bootError" class="standalone">
    <div class="drag-bar"><WindowControls /></div>
    <SetupScreen v-if="setupMode" />
    <div v-else class="boot-error">
      <p>{{ bootError }}</p>
      <p>请从 hawk 桌面端启动本应用</p>
    </div>
  </div>

  <!-- Eagle 式布局：侧栏/检查器通高，标题栏只覆盖中栏；窗口控制 fixed 于窗口右上角（Windows/Linux） -->
  <div
    v-else
    class="app"
    :class="{ 'no-panels': !store.sidebarVisible }"
    :style="{
      gridTemplateColumns: store.sidebarVisible
        ? `${sidebarWidth}px 1fr ${inspectorWidth}px`
        : '0 1fr 0',
    }"
  >
    <Sidebar class="sidebar" />
    <TitleBar class="titlebar" />
    <ItemGrid />
    <!-- 缩略图后台积压指示：细进度条压在网格顶缘（浏览器式加载条），计数归零自动消失 -->
    <div v-if="store.taskBacklog" class="task-bar">
      <div class="task-bar-fill" />
      <span class="task-bar-text">正在生成缩略图 · 剩余 {{ store.taskBacklog.pending + store.taskBacklog.active }}</span>
    </div>
    <Inspector class="inspector" />
    <WindowControls />

    <!-- 侧栏宽度拖拽手柄：4px 命中区紧贴分界线右侧，避开左侧面板的滚动条 -->
    <template v-if="store.sidebarVisible">
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

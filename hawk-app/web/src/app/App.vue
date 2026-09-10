<script setup lang="ts">
// 装配根组件：相位分发（启动/引导/门页/错误 ↔ 主界面）与布局模板。
// 启动状态机在 app/boot.ts，面板拖拽在 app/panelResize.ts，本组件只做接线与胶水。
import { ref, watch } from 'vue';
import { apiConfig, clearStoredToken } from '@/shared/api/client';
import { useLibraryStore } from '@/stores/library';
import { useImporterStore } from '@/stores/importer';
import { usePreviewStore } from '@/stores/preview';
import { useShortcuts } from './shortcuts';
import { useDragImport } from '@/composables/useDragImport';
import { useLayout } from '@/shared/composables/useLayout';
import { useBoot } from './boot';
import { usePanelResize } from './panelResize';
import { startupAutoCheck } from '@/composables/useUpdater';
import { hasShell, shell } from '@/shared/lib/platform';
import Sidebar from '@/components/Sidebar.vue';
import TitleBar from '@/app/chrome/TitleBar.vue';
import FilterBar from '@/components/FilterBar.vue';
import WindowControls from '@/app/chrome/WindowControls.vue';
import ItemGrid from '@/components/ItemGrid.vue';
import Inspector from '@/components/Inspector.vue';
import PreviewOverlay from '@/components/PreviewOverlay.vue';
import ImageEditDialog from '@/components/ImageEditDialog.vue';
import ContextMenu from '@/shared/ui/ContextMenu.vue';
import SetupScreen from './screens/SetupScreen.vue';
import ConnectScreen from './screens/ConnectScreen.vue';
import StartingScreen from './screens/StartingScreen.vue';
import SettingsDialog from '@/components/SettingsDialog.vue';
import ImportDuplicateDialog from '@/components/ImportDuplicateDialog.vue';

const store = useLibraryStore();
const importer = useImporterStore();
const preview = usePreviewStore();
const { narrow, touch } = useLayout();
const { phase, bootError, progress, runBoot } = useBoot();
const { sidebarWidth, inspectorWidth, dragSide, startResize } = usePanelResize();
// 窄屏下侧栏为抽屉式：进入窄屏默认收起（抽屉关闭），回宽屏恢复双栏
watch(
  narrow,
  (isNarrow) => {
    store.sidebarVisible = !isNarrow;
  },
  { immediate: true },
);
const showSettings = ref(false);

// 页面标题：hawk | 素材库名（浏览器标签页；Electron 下同为本窗口标题，任务栏/切换窗口可见）。
// 库未加载/门页阶段保持纯 hawk
watch(
  () => store.library?.name,
  (name) => {
    document.title = name ? `hawk | ${name}` : 'hawk';
  },
  { immediate: true },
);

// ConnectScreen 验证通过：token 已注入，重回 starting。浏览器靠轮询就绪后 runBoot；
// Electron 的 server 必已就绪（token 由主进程持有，401 场景实际不可达），直接 boot
function onConnected() {
  phase.value = 'starting';
  if (hasShell) {
    void runBoot();
  }
}

function quitApp() {
  void shell.quitApp();
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
  if ((e.target as HTMLElement).closest('.entry, .node, .tax-row')) {
    store.toggleSidebar();
  }
}

// 主界面就绪后触发一次启动静默检查（延迟 8s，每会话一次；见 useUpdater）
watch(phase, (p) => {
  if (p === 'ready') {
    startupAutoCheck();
  }
});

useShortcuts();
useDragImport();
</script>

<template>
  <!-- 启动/引导/门页/错误：单页内的前置阶段；无边框窗口下仍需拖拽区与窗口控制按钮 -->
  <div v-if="phase !== 'ready'" class="standalone">
    <div class="drag-bar"><WindowControls /></div>
    <StartingScreen v-if="phase === 'starting' || phase === 'error'" :progress="progress" :error="phase === 'error' ? bootError : null" @quit="quitApp" />
    <SetupScreen v-else-if="phase === 'setup'" @selected="phase = 'starting'" />
    <ConnectScreen v-else-if="phase === 'connect'" @connect="onConnected" />
  </div>

  <!-- Eagle 式布局：侧栏/检查器通高，标题栏只覆盖中栏；窗口控制 fixed 于窗口右上角（Windows/Linux） -->
  <div
    v-else
    class="app"
    :class="{ 'no-panels': !store.sidebarVisible, mobile: narrow, 'drawer-open': narrow && store.sidebarVisible }"
    :style="{
      gridTemplateColumns: narrow ? 'minmax(0, 1fr)' : store.sidebarVisible ? `${sidebarWidth}px minmax(0, 1fr) ${inspectorWidth}px` : '0 minmax(0, 1fr) 0',
    }"
  >
    <!-- display:contents 包裹层仅用于移动端「导航后收起抽屉」的点击委托，不改变网格布局 -->
    <div class="sidebar-wrap" @click="onSidebarNav">
      <Sidebar class="sidebar" />
    </div>
    <!-- 窄屏抽屉遮罩：点按空白处收起 -->
    <div v-if="narrow && store.sidebarVisible" class="drawer-scrim" @click="store.toggleSidebar()" />
    <TitleBar class="titlebar" @open-settings="showSettings = true" />
    <!-- 筛选工具列：顶栏漏斗按钮展开，或评分/颜色/尺寸条件激活时常驻 -->
    <FilterBar v-if="store.filterBarVisible || store.hasActiveFilters" />
    <ItemGrid />
    <!-- 索引进度指示：入库队列/扫描进度（与缩略图条同为只读指示） -->
    <div v-if="store.indexProgress" class="task-bar index">
      <div class="task-bar-fill" />
      <span class="task-bar-text">{{ store.indexProgressText }}</span>
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
      <div class="col-resize-handle" :class="{ active: dragSide === 'left' }" :style="{ left: `${sidebarWidth}px` }" @mousedown.prevent="startResize('left')" />
      <div
        class="col-resize-handle"
        :class="{ active: dragSide === 'right' }"
        :style="{ left: `calc(100% - ${inspectorWidth}px)` }"
        @mousedown.prevent="startResize('right')"
      />
    </template>

    <PreviewOverlay v-if="preview.previewItem" :item="preview.previewItem" @close="preview.closePreview()" @navigate="preview.navigatePreview($event)" />
    <!-- 图片编辑窗口:网格/预览浮层右键「编辑图片…」打开,层级高于预览浮层 -->
    <ImageEditDialog v-if="preview.editorTarget" :item="preview.editorTarget" @close="preview.closeEditor()" />
    <SettingsDialog v-if="showSettings" @close="showSettings = false" @logout="logoutToken" />
    <!-- 导入重复策略对话框（导入中首个重复内容触发，选择整批生效） -->
    <ImportDuplicateDialog />
    <!-- 多位置删除策略对话框（删除含多位置副本的素材时触发） -->
    <ContextMenu />

    <Teleport to="body">
      <!-- 导入进度：拖拽落下即显示（收集文件阶段为不定态），逐个处理完推进 -->
      <div v-if="importer.importProgress" class="import-progress">
        <span class="import-progress-text">
          {{ importer.importProgress.total > 0 ? `正在导入 ${importer.importProgress.done} / ${importer.importProgress.total}` : '正在收集文件…' }}
        </span>
        <div class="import-progress-track">
          <div
            class="import-progress-bar"
            :class="{ indeterminate: importer.importProgress.total === 0 }"
            :style="importer.importProgress.total > 0 ? { width: `${(importer.importProgress.done / importer.importProgress.total) * 100}%` } : undefined"
          />
        </div>
      </div>
      <div v-if="store.toast" class="toast" :class="{ 'toast-raised': importer.importProgress }">{{ store.toast }}</div>
    </Teleport>
  </div>
</template>

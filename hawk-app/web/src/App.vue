<script setup lang="ts">
import { onMounted, ref } from 'vue';
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
import ContextMenu from './components/ContextMenu.vue';
import SetupScreen from './components/SetupScreen.vue';

const store = useLibraryStore();
const bootError = ref<string | null>(null);
// 无连接参数但在 Electron 内：素材库未配置，进引导页
const setupMode = ref(false);

onMounted(async () => {
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
  connectEvents({
    onAdded: (item) => store.applyEvent('item.added', item),
    onUpdated: (item) => store.applyEvent('item.updated', item),
    onTrashed: (id) => store.applyEvent('item.trashed', { id }),
    onRestored: (item) => store.applyEvent('item.restored', item),
    onRemoved: (id) => store.applyEvent('item.removed', { id }),
    onReconnect: () => {
      void store.refresh();
      void store.refreshFolders();
    },
  });
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

  <div v-else class="app" :class="{ 'no-sidebar': !store.sidebarVisible }">
    <TitleBar class="titlebar" />
    <Sidebar class="sidebar" />
    <ItemGrid />
    <Inspector class="inspector" />

    <PreviewOverlay
      v-if="store.previewItem"
      :item="store.previewItem"
      @close="store.closePreview()"
      @navigate="store.navigatePreview($event)"
    />
    <ContextMenu />

    <Teleport to="body">
      <div v-if="store.toast" class="toast">{{ store.toast }}</div>
    </Teleport>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { initApi } from './api/client';
import { connectEvents } from './api/events';
import { useLibraryStore } from './stores/library';
import { useShortcuts } from './composables/useShortcuts';
import { useDragImport } from './composables/useDragImport';
import Sidebar from './components/Sidebar.vue';
import Toolbar from './components/Toolbar.vue';
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
  <SetupScreen v-if="setupMode" />

  <div v-else-if="bootError" class="boot-error">
    <p>{{ bootError }}</p>
    <p>请从 hawk 桌面端启动本应用</p>
  </div>

  <div v-else class="app">
    <Sidebar class="sidebar" />
    <Toolbar class="toolbar" />
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

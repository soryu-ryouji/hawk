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

const store = useLibraryStore();
const bootError = ref<string | null>(null);

onMounted(async () => {
  if (!initApi()) {
    bootError.value = '缺少后端连接参数';
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
  <div v-if="bootError" class="boot-error">
    <p>{{ bootError }}</p>
    <p>请从 hawk 桌面端启动本应用</p>
  </div>

  <div v-else class="app">
    <Sidebar class="sidebar" />
    <Toolbar class="toolbar" />
    <ItemGrid />
    <Inspector class="inspector" />

    <div class="statusbar">
      共 {{ store.total }} 项
      <span v-if="store.selection.length">　已选 {{ store.selection.length }} 项</span>
      <span v-if="store.isTrash">　回收站</span>
    </div>

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

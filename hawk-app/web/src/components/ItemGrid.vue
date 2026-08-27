<script setup lang="ts">
import { ref } from 'vue';
import { useIntersectionObserver } from '@vueuse/core';
import { useLibraryStore } from '../stores/library';
import { useContextMenu } from '../composables/useContextMenu';
import type { Item } from '../types';
import ItemCard from './ItemCard.vue';
import EmptyState from './EmptyState.vue';
import PromptDialog from './PromptDialog.vue';
import FolderPickerDialog from './FolderPickerDialog.vue';
import CategoryPickerDialog from './CategoryPickerDialog.vue';

const store = useLibraryStore();
const menu = useContextMenu();

const showTagDialog = ref(false);
const showFolderDialog = ref(false);
const showCategoryDialog = ref(false);

/** 为全部选中项追加标签（去重） */
function addTagToSelected(tag: string) {
  for (const id of store.selection) {
    const item = store.items.find((i) => i.id === id);
    if (item && !(item.tags ?? []).includes(tag)) {
      void store.updateItem(id, { tags: [...(item.tags ?? []), tag] });
    }
  }
}

/** 将选中项移动到目标文件夹（空字符串为根目录） */
function moveSelectedToFolder(path: string) {
  for (const id of store.selection) {
    void store.updateItem(id, { folder_path: path });
  }
}

const sentinel = ref<HTMLElement | null>(null);
useIntersectionObserver(sentinel, ([entry]) => {
  if (entry.isIntersecting) {
    void store.fetchMore();
  }
});

function onSelect(item: Item, e: MouseEvent) {
  const mod = e.shiftKey ? 'range' : e.metaKey || e.ctrlKey ? 'toggle' : undefined;
  store.select(item.id, mod);
}

function confirmClearTrash() {
  if (window.confirm('彻底删除回收站中的全部素材？此操作不可恢复。')) {
    void store.clearTrash();
  }
}

function onMenu(item: Item, e: MouseEvent) {
  // 右键未选中项时先选中它
  if (!store.selection.includes(item.id)) {
    store.select(item.id);
  }

  const items = store.isTrash
    ? [
        { label: '恢复', action: () => void store.restoreSelected() },
        { label: '清空回收站', danger: true, action: confirmClearTrash },
      ]
    : [
        { label: '添加标签…', action: () => (showTagDialog.value = true) },
        { label: '添加到分类…', action: () => (showCategoryDialog.value = true) },
        { label: '移动到文件夹…', action: () => (showFolderDialog.value = true) },
        { label: '在 Finder 中显示', action: () => window.hawkShell?.showInFinder(item.paths[0]) },
        { separator: true, label: '' },
        ...[5, 4, 3, 2, 1, 0].map((star) => ({
          label: `评分 ${star} 星`,
          action: () => {
            for (const id of store.selection) {
              void store.updateItem(id, { star });
            }
          },
        })),
        { separator: true, label: '' },
        { label: '移入回收站', danger: true, action: () => void store.trashSelected() },
      ];

  menu.open(items, e);
}
</script>

<template>
  <div class="grid-scroll">
    <EmptyState
      v-if="!store.loading && store.total === 0"
      :text="store.isTrash ? '回收站为空' : '暂无素材，拖入文件开始'"
    />

    <div class="grid" :style="{ '--thumb-size': store.thumbSize + 'px' }">
      <ItemCard
        v-for="item in store.items"
        :key="item.id"
        :item="item"
        :selected="store.selection.includes(item.id)"
        @select="onSelect"
        @open="store.openPreview"
        @menu="onMenu"
      />
    </div>

    <div ref="sentinel" class="sentinel" />
    <div v-if="store.loading" class="loading">加载中…</div>

    <PromptDialog
      v-if="showTagDialog"
      title="添加标签"
      placeholder="输入标签，回车确认"
      :suggestions="store.tagList.map((t) => t.name)"
      @confirm="
        addTagToSelected($event);
        showTagDialog = false;
      "
      @cancel="showTagDialog = false"
    />
    <CategoryPickerDialog
      v-if="showCategoryDialog"
      title="添加到分类"
      @confirm="
        store.addCategoryToSelected($event);
        showCategoryDialog = false;
      "
      @cancel="showCategoryDialog = false"
    />
    <FolderPickerDialog
      v-if="showFolderDialog"
      title="移动到文件夹"
      @confirm="
        moveSelectedToFolder($event);
        showFolderDialog = false;
      "
      @cancel="showFolderDialog = false"
    />

    <!-- 回收站工具条 -->
    <div v-if="store.isTrash && store.total > 0" class="trash-bar">
      <button class="danger" @click="confirmClearTrash">清空回收站</button>
    </div>
  </div>
</template>

<style scoped>
.grid-scroll {
  position: relative;
  overflow-y: auto;
  background: var(--bg-0);
  padding: 12px;
}

.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(var(--thumb-size), 1fr));
  gap: 10px;
}

.sentinel {
  height: 1px;
}

.loading {
  padding: 16px;
  text-align: center;
  color: var(--fg-1);
}

.trash-bar {
  position: sticky;
  bottom: 12px;
  display: flex;
  justify-content: flex-end;
  padding-top: 12px;
}
</style>

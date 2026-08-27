<script setup lang="ts">
import { computed, ref, watch, watchEffect } from 'vue';
import { useIntersectionObserver, useResizeObserver } from '@vueuse/core';
import { useLibraryStore } from '../stores/library';
import { useContextMenu } from '../composables/useContextMenu';
import { gridNavRows } from '../composables/useGridNav';
import { showInFileManagerLabel } from '../platform';
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

// ---------- 齐行网格（justified layout）：行内等高、宽度按宽高比分配，图片完整显示 ----------

interface GridCell {
  item: Item;
  width: number;
  height: number;
}

const GAP = 10;
const gridRef = ref<HTMLElement | null>(null);
const containerWidth = ref(0);

useResizeObserver(gridRef, ([entry]) => {
  containerWidth.value = entry.contentRect.width;
});

/** 贪心装行：累计到超出容器即切行；非末行按容器宽精确反推行高（上下限避免极端行） */
const gridRows = computed<GridCell[][]>(() => {
  const width = containerWidth.value;
  if (width <= 0) {
    return [];
  }

  const targetH = store.thumbSize;
  const rows: GridCell[][] = [];
  let row: { item: Item; ratio: number }[] = [];
  let ratiosSum = 0;

  const flush = (isLast: boolean) => {
    if (row.length === 0) {
      return;
    }
    const h = isLast
      ? targetH
      : Math.min(Math.max((width - (row.length - 1) * GAP) / ratiosSum, targetH * 0.5), targetH * 1.75);
    rows.push(row.map(({ item, ratio }) => ({ item, width: Math.round(h * ratio), height: Math.round(h) })));
    row = [];
    ratiosSum = 0;
  };

  for (const item of store.items) {
    const ratio = Number(item.width) > 0 && Number(item.height) > 0 ? Number(item.width) / Number(item.height) : 1;
    if (row.length > 0 && (ratiosSum + ratio) * targetH + row.length * GAP > width) {
      flush(false);
    }
    row.push({ item, ratio });
    ratiosSum += ratio;
  }
  flush(true);
  return rows;
});

// 发布方向键导航所需的行布局（每项的视觉中心 x）
watchEffect(() => {
  gridNavRows.value = gridRows.value.map((row) => {
    let x = 0;
    return row.map((cell) => {
      const cx = x + cell.width / 2;
      x += cell.width + GAP;
      return { id: cell.item.id, cx };
    });
  });
});

// 键盘移动选中框时滚动到可见区域
watch(
  () => store.primarySelected?.id,
  (id) => {
    if (id) {
      document.querySelector(`[data-item-id="${id}"]`)?.scrollIntoView({ block: 'nearest' });
    }
  },
);

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
        { label: showInFileManagerLabel, action: () => window.hawkShell?.showInFinder(item.paths[0]) },
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

    <div ref="gridRef" class="grid">
      <div v-for="(row, i) in gridRows" :key="i" class="row">
        <ItemCard
          v-for="cell in row"
          :key="cell.item.id"
          :item="cell.item"
          :data-item-id="cell.item.id"
          :selected="store.selection.includes(cell.item.id)"
          :width="cell.width"
          :height="cell.height"
          @select="onSelect"
          @open="store.openPreview"
          @menu="onMenu"
        />
      </div>
    </div>

    <div ref="sentinel" class="sentinel" />
    <div v-if="store.loading" class="loading">加载中…</div>

    <PromptDialog
      v-if="showTagDialog"
      title="添加标签"
      placeholder="输入标签，回车确认"
      :suggestions="store.tagList.map((t) => t.name)"
      @confirm="
        store.addTagToSelected($event);
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
        store.moveSelectedToFolder($event);
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
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.row {
  display: flex;
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

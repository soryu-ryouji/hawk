<script setup lang="ts">
import { computed, nextTick, ref, watch, watchEffect } from 'vue';
import { useResizeObserver } from '@vueuse/core';
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
import { isRotatableImage } from '../imageEdit';

const store = useLibraryStore();
const menu = useContextMenu();

const showTagDialog = ref(false);
const showFolderDialog = ref(false);
const showCategoryDialog = ref(false);

// ---------- 齐行布局（justified layout）+ 虚拟渲染 ----------
// Eagle 式：骨架（全量 id/宽/高）一次性算出完整布局，滚动条总高即时确定、可自由拖动；
// 只渲染视口 ± overscan 的行，行内详情未拉取的单元格只保留宽高的占位块（不渲染图片）。

interface LayoutCell {
  id: string;
  width: number;
  height: number;
  star: number;
}

interface LayoutRow {
  key: string;
  cells: LayoutCell[];
  y: number;
  height: number;
  /** 行内条目在骨架中的索引区间 [startIdx, endIdx)，视口窗口按它向 store 补数据 */
  startIdx: number;
  endIdx: number;
}

const GAP = 10;
/** 视口外行缓存：上下各多渲染的行数，吸收快速滚动的渲染延迟 */
const OVERSCAN_ROWS = 4;
/** 卡片 meta 区定高（Eagle 式 3 行：标题 2 + 像素 1），必须与 ItemCard.vue 中 .meta 的 height 一致 */
const META_H = 54;
/** 卡片边框 2px×2：行距必须计入，否则下一行图片盖住上一行的 meta 文字 */
const CARD_BORDER = 4;

const gridRef = ref<HTMLElement | null>(null);
const containerWidth = ref(0);
const viewportHeight = ref(0);
const scrollTop = ref(0);

useResizeObserver(gridRef, ([entry]) => {
  containerWidth.value = entry.contentRect.width;
  viewportHeight.value = entry.contentRect.height;
});

let scrollRaf = 0;
function onScroll() {
  if (scrollRaf) {
    return;
  }
  scrollRaf = requestAnimationFrame(() => {
    scrollRaf = 0;
    scrollTop.value = gridRef.value?.scrollTop ?? 0;
  });
}

/** 贪心装行：累计到超出容器即切行；非末行按容器宽精确反推行高（上下限避免极端行）
 * 只依赖骨架 + 卡片尺寸 + 容器宽：详情缓存变化不触发全量重排（大库上每次窗口拉取都重排太贵） */
const layout = computed<LayoutRow[]>(() => {
  const width = containerWidth.value;
  if (width <= 0) {
    return [];
  }

  const targetH = store.thumbSize;
  const sk = store.skeleton;
  const rows: LayoutRow[] = [];
  let y = 0;
  let row: { idx: number; id: string; ratio: number; star: number }[] = [];
  let ratiosSum = 0;

  const flush = (isLast: boolean) => {
    if (row.length === 0) {
      return;
    }
    const h = Math.round(
      isLast ? targetH : Math.min(Math.max((width - (row.length - 1) * GAP) / ratiosSum, targetH * 0.5), targetH * 1.75),
    );
    // 行高 = 卡片总高（缩略图 + meta + 边框），行槽位与真实卡片一致，杜绝行间重叠
    const rowH = h + META_H + CARD_BORDER;
    rows.push({
      key: row[0].id,
      cells: row.map((r) => ({ id: r.id, width: Math.round(h * r.ratio), height: h, star: r.star })),
      y,
      height: rowH,
      startIdx: row[0].idx,
      endIdx: row[row.length - 1].idx + 1,
    });
    y += rowH + GAP;
    row = [];
    ratiosSum = 0;
  };

  for (let idx = 0; idx < sk.length; idx++) {
    const s = sk[idx];
    const ratio = Number(s.width) > 0 && Number(s.height) > 0 ? Number(s.width) / Number(s.height) : 1;
    if (row.length > 0 && (ratiosSum + ratio) * targetH + row.length * GAP > width) {
      flush(false);
    }
    row.push({ idx, id: s.id, ratio, star: Number(s.star) });
    ratiosSum += ratio;
  }
  flush(true);
  return rows;
});

/** 滚动条总高：布局完成即确定，不随滚动变化 */
const totalHeight = computed(() => {
  const rows = layout.value;
  return rows.length > 0 ? rows[rows.length - 1].y + rows[rows.length - 1].height : 0;
});

/** 可见行区间（按 y 二分），上下各扩 OVERSCAN_ROWS 行 */
const visibleRange = computed<[number, number]>(() => {
  const rows = layout.value;
  if (rows.length === 0) {
    return [0, -1];
  }
  const top = scrollTop.value;
  const bottom = scrollTop.value + viewportHeight.value;
  let lo = 0;
  let hi = rows.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (rows[mid].y + rows[mid].height < top) {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }
  let end = lo;
  while (end + 1 < rows.length && rows[end + 1].y <= bottom) {
    end++;
  }
  return [Math.max(0, lo - OVERSCAN_ROWS), Math.min(rows.length - 1, end + OVERSCAN_ROWS)];
});

interface RenderedCell extends LayoutCell {
  item: Item | null;
}

/** 视口行 + 详情解析：只在这个切片上映射 details，详情到位后占位块换成真实卡片 */
const renderedRows = computed(() => {
  const [a, b] = visibleRange.value;
  if (a > b) {
    return [];
  }
  return layout.value.slice(a, b + 1).map((row) => ({
    ...row,
    cells: row.cells.map((c): RenderedCell => ({ ...c, item: store.details.get(c.id) ?? null })),
  }));
});

// 视口窗口补数据（首屏由 resetList 负责，这里兜滚动/跳转）
watchEffect(() => {
  const [a, b] = visibleRange.value;
  const rows = layout.value;
  if (a <= b && a < rows.length) {
    void store.ensureWindow(rows[a].startIdx, rows[b].endIdx);
  }
});

// 发布方向键导航所需的行布局（每项的视觉中心 x）
watchEffect(() => {
  gridNavRows.value = layout.value.map((row) => {
    let x = 0;
    return row.cells.map((cell) => {
      const cx = x + cell.width / 2;
      x += cell.width + GAP;
      return { id: cell.id, cx };
    });
  });
});

// 键盘移动选中框时滚动到可见区域；目标行未渲染时先把容器滚过去（触发渲染）再细调
watch(
  () => store.primarySelected?.id,
  async (id) => {
    if (!id || !gridRef.value) {
      return;
    }
    const idx = store.skeleton.findIndex((s) => s.id === id);
    if (idx < 0) {
      return;
    }
    const row = layout.value.find((r) => idx >= r.startIdx && idx < r.endIdx);
    const el = gridRef.value;
    if (row && (row.y < el.scrollTop || row.y + row.height > el.scrollTop + el.clientHeight)) {
      el.scrollTop = Math.max(0, row.y - 100);
    }
    await nextTick();
    requestAnimationFrame(() => {
      document.querySelector(`[data-item-id="${id}"]`)?.scrollIntoView({ block: 'nearest' });
    });
  },
);

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
  // 只读查看（局域网 viewer）：写入口整体隐藏,不出右键菜单
  if (store.viewerMode) {
    return;
  }
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
        // 编辑仅支持 canvas 可重编码的格式(见 imageEdit.ts 白名单),其余不出现该入口
        ...(isRotatableImage(item.ext) ? [{ label: '编辑图片…', action: () => store.openEditor(item) }] : []),
        // 「在文件管理器中显示」依赖 Electron 主进程,浏览器（局域网查看）不出现
        ...(window.hawkShell ? [{ label: showInFileManagerLabel, action: () => window.hawkShell?.showInFinder(item.paths[0]) }] : []),
        { separator: true, label: '' },
        ...[5, 4, 3, 2, 1, 0].map((star) => ({
          label: `评分 ${star} 星`,
          action: () => void store.setStarForSelected(star),
        })),
        { separator: true, label: '' },
        { label: '移入回收站', danger: true, action: () => void store.trashSelected() },
      ];

  menu.open(items, e);
}
</script>

<template>
  <div ref="gridRef" class="grid-scroll" @scroll.passive="onScroll">
    <EmptyState
      v-if="!store.loading && store.total === 0"
      :text="store.isTrash ? '回收站为空' : '暂无素材，拖入文件开始'"
    />

    <div v-if="totalHeight > 0" class="grid" :style="{ height: `${totalHeight}px` }">
      <div v-for="row in renderedRows" :key="row.key" class="row" :style="{ transform: `translateY(${row.y}px)` }">
        <template v-for="cell in row.cells" :key="cell.id">
          <ItemCard
            v-if="cell.item"
            :item="cell.item"
            :data-item-id="cell.id"
            :selected="store.selection.includes(cell.id)"
            :width="cell.width"
            :height="cell.height"
            @select="onSelect"
            @open="store.openPreview"
            @menu="onMenu"
          />
          <!-- 详情未拉取：只保留宽高的占位块，不进视口渲染（ Eagle 式） -->
          <div
            v-else
            class="cell-placeholder"
            :class="{ selected: store.selection.includes(cell.id) }"
            :style="{ width: `${cell.width}px`, height: `${cell.height + META_H}px` }"
            :data-item-id="cell.id"
          >
            <span v-if="cell.star > 0" class="star">★{{ cell.star }}</span>
          </div>
        </template>
      </div>
    </div>

    <div v-if="store.windowLoading && !store.loading" class="loading">加载中…</div>

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

/* 总高由内联 style 决定；行绝对定位 + translateY，离屏行不渲染 */
.grid {
  position: relative;
}

.row {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  display: flex;
  gap: 10px;
}

.cell-placeholder {
  position: relative;
  flex: none;
  border-radius: 4px;
  background: var(--bg-2);
  border: 2px solid transparent;
}

.cell-placeholder.selected {
  border-color: var(--accent);
}

.cell-placeholder .star {
  position: absolute;
  right: 5px;
  top: 4px;
  color: #f5c518;
  font-size: 11px;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.8);
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

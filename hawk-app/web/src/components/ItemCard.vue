<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { api } from '../api/endpoints';
import { startItemsDrag } from '../dnd';
import { useLibraryStore } from '../stores/library';
import { CARD_BORDER, CARD_META_H } from '../layout';
import type { Item } from '../types';

const props = withDefaults(
  defineProps<{ item: Item; selected: boolean; width?: number; height?: number }>(),
  { width: 0, height: 0 },
);
const emit = defineEmits<{
  select: [item: Item, e: MouseEvent];
  open: [id: string];
  menu: [item: Item, e: MouseEvent];
}>();

const store = useLibraryStore();
const thumbFailed = ref(false);

// 缩略图就绪后后端补发 item.updated：item 引用变化时重建 <img> 重试（此前可能 404 占位）
watch(
  () => props.item,
  () => {
    thumbFailed.value = false;
  },
);

/** 齐行网格传入的单元尺寸（宽高比与图片一致，图片完整显示不裁切） */
const thumbStyle = computed(() =>
  props.width && props.height
    ? { width: props.width + 'px', height: props.height + 'px' }
    : { width: '100%', aspectRatio: '1' },
);

/** 卡片宽度锁定为缩略图宽度：长名称不得撑开卡片（名称走 ellipsis 截断）。
 * 卡片样式常量（meta 高/边框）经 CSS 变量下发，与 ItemGrid 的行距数学同一来源（layout.ts） */
const cardStyle = computed(() =>
  props.width
    ? {
        width: props.width + 'px',
        '--meta-h': `${CARD_META_H}px`,
        '--card-border': `${CARD_BORDER}px`,
      }
    : { '--meta-h': `${CARD_META_H}px`, '--card-border': `${CARD_BORDER}px` },
);

// 缩略图单尺寸 1024（内容寻址、immutable），小图由浏览器缩小显示
const thumbSrc = computed(() => api.thumbnailUrl(props.item.id));

/**
 * 拖到侧栏（文件夹/分类/标签）的拖拽源。Eagle 语义：拖未选中的项 → 改为单选它；
 * 拖已选中的项 → 带动整个选择集。回收站视图禁止拖出（服务端也拒绝移动回收站文件）。
 */
function onDragStart(e: DragEvent) {
  if (!props.selected) {
    store.select(props.item.id);
  }
  startItemsDrag(e, store.selection);
}
</script>

<template>
  <div
    class="card"
    :class="{ selected }"
    :style="cardStyle"
    :draggable="!store.isTrash"
    @click="emit('select', item, $event)"
    @dblclick="emit('open', item.id)"
    @contextmenu.prevent="emit('menu', item, $event)"
    @dragstart="onDragStart"
  >
    <div class="thumb" :style="thumbStyle">
      <img
        v-if="!thumbFailed"
        :src="thumbSrc"
        :alt="item.name"
        loading="lazy"
        draggable="false"
        @error="thumbFailed = true"
      />
      <div v-else class="placeholder">{{ item.ext || '?' }}</div>
      <span v-if="Number(item.star) > 0" class="star">★{{ item.star }}</span>
    </div>
    <div class="meta">
      <div class="name" :title="`${item.name}.${item.ext}`">{{ item.name }}.{{ item.ext }}</div>
      <div class="dims">{{ item.width }} × {{ item.height }}</div>
    </div>
  </div>
</template>

<style scoped>
.card {
  flex: none; /* 行内宽度由齐行网格精确分配，不允许伸缩 */
  border-radius: 4px;
  overflow: hidden;
  background: var(--bg-2);
  /* 总边框宽 = --card-border（2px × 2），与 ItemGrid 行槽位计算同一来源 */
  border: calc(var(--card-border, 4px) / 2) solid transparent;
  cursor: default;
}

.card.selected {
  border-color: var(--accent);
}

.thumb {
  position: relative;
  overflow: hidden;
  background: #171717;
}

/* Eagle 观感：单元格与图片同宽高比，完整显示 */
.thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}

.placeholder {
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--fg-1);
  font-size: 20px;
  text-transform: uppercase;
}

.star {
  position: absolute;
  right: 5px;
  top: 4px;
  color: #f5c518;
  font-size: 11px;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.8);
}

.meta {
  /* 定高（--meta-h）：行距按它计算，与 ItemGrid 的 CARD_META_H 同一来源； Eagle 式预留 3 行（标题 2 + 像素 1） */
  height: var(--meta-h, 54px);
  box-sizing: border-box;
  padding: 5px 7px 6px;
  background: var(--bg-2);
  text-align: center;
}

/* 标题最多两行，超出省略（line-clamp 替代 nowrap + text-overflow） */
.name {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  color: var(--fg-0);
  font-size: 12px;
  line-height: 14px;
  word-break: break-all;
}

.dims {
  margin-top: 2px;
  color: var(--fg-1);
  font-size: 11px;
  line-height: 13px;
}
</style>

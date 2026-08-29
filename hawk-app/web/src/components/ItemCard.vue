<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
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

/** 卡片宽度锁定为缩略图宽度：长名称不得撑开卡片（名称走 ellipsis 截断） */
const cardStyle = computed(() => (props.width ? { width: props.width + 'px' } : {}));

// 缩略图 srcset：候选尺寸来自服务端 thumbnail_sizes（内容寻址、immutable，浏览器按 渲染宽 × DPR 选档）
const thumbSrcSet = computed(() => store.thumbSizes.map((s) => `${api.thumbnailUrl(props.item.id, s)} ${s}w`).join(', '));
// src 兜底取 ≥512 的最近档（无则最大档）：srcset 生效时浏览器忽略 src
const thumbSrc = computed(
  () => api.thumbnailUrl(props.item.id, store.thumbSizes.find((s) => s >= 512) ?? store.thumbSizes.at(-1) ?? 256),
);
/** sizes 声明 img 的 CSS 渲染宽（齐行网格传入的单元格宽），浏览器据此 × DPR 从 srcset 选档 */
const thumbSizesAttr = computed(() => (props.width > 0 ? `${Math.ceil(props.width)}px` : '100vw'));
</script>

<template>
  <div
    class="card"
    :class="{ selected }"
    :style="cardStyle"
    @click="emit('select', item, $event)"
    @dblclick="emit('open', item.id)"
    @contextmenu.prevent="emit('menu', item, $event)"
  >
    <div class="thumb" :style="thumbStyle">
      <img
        v-if="!thumbFailed"
        :src="thumbSrc"
        :srcset="thumbSrcSet"
        :sizes="thumbSizesAttr"
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
  border: 2px solid transparent;
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
  /* 定高：ItemGrid.vue 的 META_H 常量与此保持一致，行距按它计算； Eagle 式预留 3 行（标题 2 + 像素 1） */
  height: 54px;
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

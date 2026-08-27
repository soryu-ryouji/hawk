<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { api } from '../api/endpoints';
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
        :src="api.thumbnailUrl(item.id)"
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
  padding: 5px 7px 6px;
  background: var(--bg-2);
}

.name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg-0);
  font-size: 12px;
}

.dims {
  margin-top: 1px;
  color: var(--fg-1);
  font-size: 11px;
}
</style>

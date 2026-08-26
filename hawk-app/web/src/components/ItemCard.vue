<script setup lang="ts">
import { ref, watch } from 'vue';
import { api } from '../api/endpoints';
import type { Item } from '../types';

const props = defineProps<{ item: Item; selected: boolean }>();
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
</script>

<template>
  <div
    class="card"
    :class="{ selected }"
    @click="emit('select', item, $event)"
    @dblclick="emit('open', item.id)"
    @contextmenu.prevent="emit('menu', item, $event)"
  >
    <div class="thumb">
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
    <div class="name" :title="item.name">{{ item.name }}</div>
  </div>
</template>

<style scoped>
.card {
  border-radius: 6px;
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
  aspect-ratio: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}

.thumb img {
  max-width: 100%;
  max-height: 100%;
  object-fit: contain;
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
  right: 6px;
  bottom: 4px;
  color: #f5c518;
  font-size: 11px;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.6);
}

.name {
  padding: 6px 8px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--fg-0);
}
</style>

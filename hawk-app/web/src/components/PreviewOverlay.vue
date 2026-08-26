<script setup lang="ts">
import { computed } from 'vue';
import { api } from '../api/endpoints';
import type { Item } from '../types';

const props = defineProps<{ item: Item }>();
const emit = defineEmits<{ close: []; navigate: [step: 1 | -1] }>();

const imageUrl = computed(() => api.thumbnailUrl(props.item.id, 1024));
</script>

<template>
  <Teleport to="body">
    <div class="overlay" @click.self="emit('close')">
      <button class="nav prev" title="上一个" @click.stop="emit('navigate', -1)">‹</button>
      <img class="image" :src="imageUrl" :alt="item.name" />
      <button class="nav next" title="下一个" @click.stop="emit('navigate', 1)">›</button>
      <div class="caption">{{ item.name }}.{{ item.ext }}</div>
      <button class="close" title="关闭 (Esc)" @click="emit('close')">×</button>
    </div>
  </Teleport>
</template>

<style scoped>
.overlay {
  position: fixed;
  inset: 0;
  z-index: 200;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.85);
}

.image {
  max-width: 90vw;
  max-height: 90vh;
  object-fit: contain;
}

.close {
  position: absolute;
  top: 12px;
  right: 16px;
  border: none;
  background: transparent;
  color: var(--fg-1);
  font-size: 28px;
}

.close:hover {
  color: #fff;
  background: transparent;
}

.nav {
  position: absolute;
  top: 50%;
  transform: translateY(-50%);
  border: none;
  background: transparent;
  color: var(--fg-1);
  font-size: 48px;
  padding: 0 16px;
}

.nav:hover {
  color: #fff;
  background: transparent;
}

.nav.prev {
  left: 8px;
}

.nav.next {
  right: 8px;
}

.caption {
  position: absolute;
  bottom: 16px;
  left: 50%;
  transform: translateX(-50%);
  color: var(--fg-1);
}
</style>

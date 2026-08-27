<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { api } from '../api/endpoints';
import type { Item } from '../types';

const props = defineProps<{ item: Item }>();
const emit = defineEmits<{ close: []; navigate: [step: 1 | -1] }>();

// 预览展示原图（缩略图是压缩过的 WebP）
const imageUrl = computed(() => api.fileUrl(props.item.id));

// ---------- 缩放与平移 ----------
// scale=1 为适应窗口；滚轮以光标为不动点缩放，左键拖拽平移，双击复位
const scale = ref(1);
const tx = ref(0);
const ty = ref(0);
const dragging = ref(false);

const MIN_SCALE = 0.1;
const MAX_SCALE = 20;

const imageStyle = computed(() => ({
  transform: `translate(${tx.value}px, ${ty.value}px) scale(${scale.value})`,
  cursor: dragging.value ? 'grabbing' : scale.value > 1 ? 'grab' : 'default',
}));

watch(() => props.item.id, resetView);

function resetView() {
  scale.value = 1;
  tx.value = 0;
  ty.value = 0;
}

function onWheel(e: WheelEvent) {
  const next = Math.min(Math.max(scale.value * Math.exp(-e.deltaY * 0.002), MIN_SCALE), MAX_SCALE);
  if (next === scale.value) {
    return;
  }
  // 光标（相对视口中心）处的图像点保持不动：t' = c - k(c - t)
  const cx = e.clientX - window.innerWidth / 2;
  const cy = e.clientY - window.innerHeight / 2;
  const k = next / scale.value;
  tx.value = cx - (cx - tx.value) * k;
  ty.value = cy - (cy - ty.value) * k;
  scale.value = next;
}

let dragStart: { x: number; y: number; tx: number; ty: number } | null = null;

function onPointerDown(e: PointerEvent) {
  if (e.button !== 0) {
    return;
  }
  dragStart = { x: e.clientX, y: e.clientY, tx: tx.value, ty: ty.value };
  dragging.value = true;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
}

function onPointerMove(e: PointerEvent) {
  if (!dragStart) {
    return;
  }
  tx.value = dragStart.tx + (e.clientX - dragStart.x);
  ty.value = dragStart.ty + (e.clientY - dragStart.y);
}

function onPointerUp() {
  dragStart = null;
  dragging.value = false;
}
</script>

<template>
  <Teleport to="body">
    <div class="overlay" @click.self="emit('close')" @wheel.prevent="onWheel">
      <button class="nav prev" title="上一个" @click.stop="emit('navigate', -1)">‹</button>
      <img
        class="image"
        :src="imageUrl"
        :alt="item.name"
        :style="imageStyle"
        draggable="false"
        @pointerdown="onPointerDown"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerUp"
        @dblclick="resetView"
      />
      <button class="nav next" title="下一个" @click.stop="emit('navigate', 1)">›</button>
      <div class="caption">{{ item.name }}.{{ item.ext }} · {{ Math.round(scale * 100) }}%</div>
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
  overflow: hidden;
  background: rgba(0, 0, 0, 0.85);
}

.image {
  max-width: 90vw;
  max-height: 90vh;
  object-fit: contain;
  transform-origin: center;
  user-select: none;
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
  z-index: 1;
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

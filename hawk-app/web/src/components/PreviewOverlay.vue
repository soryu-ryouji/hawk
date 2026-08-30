<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
import { useContextMenu } from '../composables/useContextMenu';
import { isRotatableImage } from '../imageEdit';
import { showInFileManagerLabel } from '../platform';
import type { Item } from '../types';

const props = defineProps<{ item: Item }>();
const emit = defineEmits<{ close: []; navigate: [step: 1 | -1] }>();

const store = useLibraryStore();
const menu = useContextMenu();

// 底部中间序号：当前项在视图中的位置 / 视图总条目数（Eagle 式）
const indexText = computed(() => {
  const i = store.previewIndex;
  return i >= 0 ? `${i + 1} / ${store.total}` : '';
});

// 预览展示原图（缩略图是压缩过的 WebP）
const imageUrl = computed(() => api.fileUrl(props.item.id));

// 右键菜单：打开所在文件夹 / 复制文件路径 / 复制图片 / 编辑图片 / 删除
function onMenu(e: MouseEvent) {
  menu.open(
    [
      { label: showInFileManagerLabel, action: () => void window.hawkShell?.showInFinder(props.item.paths[0]) },
      { label: '复制文件路径', action: () => void window.hawkShell?.copyPath(props.item.paths[0]) },
      { label: '复制图片', action: () => void window.hawkShell?.copyImage(props.item.paths[0]) },
      // 编辑仅支持 canvas 可重编码的格式(见 imageEdit.ts 白名单),其余不出现该入口
      ...(isRotatableImage(props.item.ext) ? [{ label: '编辑图片…', action: () => store.openEditor(props.item) }] : []),
      { separator: true, label: '' },
      { label: '删除图片', danger: true, action: () => void trashCurrent() },
    ],
    e,
  );
}

// 删除当前预览项：跳到下一张（无下一张则上一张，都没有则关闭预览）
async function trashCurrent() {
  const fallback = store.previewNavId(1) ?? store.previewNavId(-1);
  store.select(props.item.id);
  await store.trashSelected();
  if (fallback && fallback !== props.item.id) {
    store.openPreview(fallback);
  } else {
    emit('close');
  }
}

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
    <div class="overlay" @click.self="emit('close')" @wheel.prevent="onWheel" @contextmenu.prevent="onMenu">
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
      <!-- 底部中间：序号 + 左右切换（Eagle 式） -->
      <div class="pager">
        <button class="page-btn" title="上一个 (←)" @click.stop="emit('navigate', -1)">‹</button>
        <span class="page-index">{{ indexText }}</span>
        <button class="page-btn" title="下一个 (→)" @click.stop="emit('navigate', 1)">›</button>
      </div>
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
  /* Eagle 式：几乎不透明的磨砂玻璃覆盖底层界面 */
  background: rgba(0, 0, 0, 0.85);
  backdrop-filter: blur(24px);
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

.pager {
  position: absolute;
  bottom: 14px;
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  align-items: center;
  gap: 18px;
  color: var(--fg-0);
}

.page-btn {
  border: none;
  background: transparent;
  color: var(--fg-1);
  font-size: 22px;
  padding: 2px 10px;
}

.page-btn:hover {
  color: #fff;
  background: transparent;
}

.page-index {
  font-size: 13px;
  min-width: 64px;
  text-align: center;
  user-select: none;
}
</style>

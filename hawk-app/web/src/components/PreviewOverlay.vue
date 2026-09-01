<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
import { usePreviewStore } from '../stores/preview';
import { useContextMenu } from '../composables/useContextMenu';
import { useLayout } from '../composables/useLayout';
import { useZoomPan } from '../composables/useZoomPan';
import { isRotatableImage } from '../imageEdit';
import { showInFileManagerLabel, hasShell, shell } from '../platform';
import type { Item } from '../types';

const props = defineProps<{ item: Item }>();
const emit = defineEmits<{ close: []; navigate: [step: 1 | -1] }>();

const store = useLibraryStore();
const preview = usePreviewStore();
const menu = useContextMenu();
const { narrow, touch } = useLayout();

// 底部中间序号：当前项在视图中的位置 / 视图总条目数（Eagle 式）
const indexText = computed(() => {
  const i = preview.previewIndex;
  return i >= 0 ? `${i + 1} / ${store.total}` : '';
});

// 预览展示原图（缩略图是压缩过的 WebP）
const imageUrl = computed(() => api.fileUrl(props.item.id));

// 右键菜单：打开所在文件夹 / 复制文件路径 / 复制图片 / 编辑图片 / 删除。
// 浏览器（无 hawkShell）隐藏系统相关项；只读查看（viewer）隐藏全部写操作，无可用项时不弹菜单。
function onMenu(e: MouseEvent) {
  const items = [
    ...(hasShell
      ? [
          { label: showInFileManagerLabel, action: () => void shell.showInFinder(props.item.paths[0]) },
          { label: '复制文件路径', action: () => void shell.copyPath(props.item.paths[0]) },
          { label: '复制图片', action: () => void shell.copyImage(props.item.paths[0]) },
        ]
      : []),
    // 编辑仅支持 canvas 可重编码的格式(见 imageEdit.ts 白名单),其余不出现该入口;viewer 下禁用
    ...(isRotatableImage(props.item.ext) && !store.viewerMode
      ? [{ label: '编辑图片…', action: () => preview.openEditor(props.item) }]
      : []),
    ...(!store.viewerMode ? [{ separator: true, label: '' }] : []),
    ...(!store.viewerMode ? [{ label: '删除图片', danger: true, action: () => void trashCurrent() }] : []),
  ];
  if (items.length === 0) {
    return;
  }
  menu.open(items, e);
}

// 删除当前预览项：跳到下一张（无下一张则上一张，都没有则关闭预览）
async function trashCurrent() {
  const fallback = preview.previewNavId(1) ?? preview.previewNavId(-1);
  store.select(props.item.id);
  await store.trashSelected();
  if (fallback && fallback !== props.item.id) {
    preview.openPreview(fallback);
  } else {
    emit('close');
  }
}

// ---------- 缩放与平移 + 滑动切换 ----------
// 手势状态机（滚轮不动点缩放/双击语义/单指平移与滑动切换/双指捏合/下拉关闭/点击边距关闭）
// 在 useZoomPan composable，那里以语义矩阵为规格逐条实现；这里只保留视觉层样式与语义接线。
const gestures = useZoomPan({
  touch,
  hasNeighbor: (dir) => preview.previewNavId(dir) !== null,
  navigate: (dir) => emit('navigate', dir),
  close: () => emit('close'),
  hitImage: pointInImage,
});
const {
  scale, tx, ty, dragging,
  swipeAnim, swipeX,
  pullActive, pullAnim, pullY,
  onWheel, onPointerDown, onPointerMove, onPointerUp, onPointerCancel, onDblClick, onGestureClick,
} = gestures;

// 移动端详情条：触屏无检查器面板且点按不开选中，预览内 ⓘ 开关底部只读详情（Eagle 信息面板的最小集）
const showInfo = ref(false);

// 预加载相邻原图：内容寻址 immutable，浏览器缓存命中——carousel 拖动时邻图已解码，切换零等待
function preloadNeighbors() {
  for (const step of [1, -1] as const) {
    const id = preview.previewNavId(step);
    if (id) {
      new Image().src = api.fileUrl(id);
    }
  }
}

onMounted(preloadNeighbors);

// carousel 邻居：id 取自骨架（不依赖详情窗口），拖动时左右邻图已经可见（iOS 相册式）
const prevId = computed(() => preview.previewNavId(-1));
const nextId = computed(() => preview.previewNavId(1));
const prevUrl = computed(() => (prevId.value ? api.fileUrl(prevId.value) : null));
const nextUrl = computed(() => (nextId.value ? api.fileUrl(nextId.value) : null));

// 缩放>1 的单图平移模式样式（carousel 模式由 trackStyle 负责）；手势层负责 cursor
const imageStyle = computed(() => ({
  transform: `translate(${tx.value}px, ${ty.value}px) scale(${scale.value})`,
}));

// carousel 轨道：三张并排（前|当前|后），基准 translateX=-100vw 使当前图居中，swipeX 为跟手偏移
const trackStyle = computed(() => ({
  transform: `translate(${swipeX.value - window.innerWidth}px, ${pullActive.value || pullAnim.value ? pullY.value : 0}px)`,
}));

// 下拉跟手时背景随位移渐亮（松手回弹/滑出后恢复默认遮罩）
const overlayStyle = computed(() => {
  if (!pullActive.value && !pullAnim.value) {
    return {};
  }
  const dim = Math.max(0, 1 - pullY.value / 420);
  return { background: `rgba(0, 0, 0, ${(0.85 * dim).toFixed(3)})` };
});

watch(() => props.item.id, () => {
  // 切图复位：手势状态机清零（手指通常已抬起，兜底防泄漏）+ 详情条收起 + 相邻预加载
  gestures.reset();
  showInfo.value = false;
  preloadNeighbors();
});

function formatSize(bytes: number): string {
  if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(2) + ' MB';
  if (bytes >= 1 << 10) return (bytes / (1 << 10)).toFixed(1) + ' KB';
  return bytes + ' B';
}

/** 平移模式图像显示区域命中测试：基准 90vw/90vh object-fit: contain，transform 以元素中心为原点 */
function pointInImage(px: number, py: number): boolean {
  const iw = Number(props.item.width);
  const ih = Number(props.item.height);
  if (!iw || !ih) {
    return true;
  }
  const fit = Math.min((window.innerWidth * 0.9) / iw, (window.innerHeight * 0.9) / ih);
  const w = iw * fit * scale.value;
  const h = ih * fit * scale.value;
  const cx = window.innerWidth / 2 + tx.value;
  const cy = window.innerHeight / 2 + ty.value;
  return Math.abs(px - cx) <= w / 2 && Math.abs(py - cy) <= h / 2;
}
</script>

<template>
  <Teleport to="body">
    <div class="overlay" :style="overlayStyle" @wheel.prevent="onWheel" @contextmenu.prevent="onMenu">
      <!--
        手势层：全屏固定、两种缩放模式共用，承接全部 pointer 手势（单指拖拽/滑动/下拉、双指捏合、点击、双击复位）。
        平移图与 carousel 轨道只是其下 pointer-events:none 的视觉层——v-if 模式切换不打断进行中的手势。
      -->
      <div
        class="gesture"
        :class="{ dragging }"
        @pointerdown="onPointerDown"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerCancel"
        @dblclick="onDblClick"
        @click="onGestureClick"
      ></div>
      <!-- 缩放>1：单图平移视觉层 -->
      <img v-if="scale > 1" class="image" :src="imageUrl" :alt="item.name" :style="imageStyle" draggable="false" />
      <!-- 缩放=1：carousel 视觉层（手势层不位移，内层轨道跟手，iOS 相册式拖动邻图可见） -->
      <div v-else class="swipe-track">
        <div class="track-row" :class="{ 'track-anim': swipeAnim || pullAnim }" :style="trackStyle">
          <img v-if="prevUrl" class="track-img" :src="prevUrl" :key="`p-${prevId}`" alt="上一张" draggable="false" />
          <div v-else class="track-img track-slot" />
          <img class="track-img" :src="imageUrl" :key="item.id" :alt="item.name" draggable="false" />
          <img v-if="nextUrl" class="track-img" :src="nextUrl" :key="`n-${nextId}`" alt="下一张" draggable="false" />
          <div v-else class="track-img track-slot" />
        </div>
      </div>
      <!-- 底部中间：序号 + 左右切换（Eagle 式） -->
      <div class="pager">
        <button class="page-btn" title="上一个 (←)" @click.stop="emit('navigate', -1)">‹</button>
        <span class="page-index">{{ indexText }}</span>
        <button class="page-btn" title="下一个 (→)" @click.stop="emit('navigate', 1)">›</button>
      </div>
      <button v-if="!touch" class="close" title="关闭 (Esc)" @click="emit('close')">×</button>
      <!-- 窄屏：ⓘ 开关底部详情条（触屏无检查器，详情只读；桌面走右侧面板不出现） -->
      <template v-if="narrow">
        <button class="info-toggle" :class="{ open: showInfo }" title="详情" @click.stop="showInfo = !showInfo">i</button>
        <div v-if="showInfo" class="info-sheet" @click.stop>
          <div class="info-name">{{ item.name }}.{{ item.ext }}</div>
          <div class="info-grid">
            <span>尺寸</span><span>{{ item.width }} × {{ item.height }}</span>
            <span>大小</span><span>{{ formatSize(Number(item.size)) }}</span>
            <span>评分</span><span>{{ Number(item.star) > 0 ? '★'.repeat(Number(item.star)) : '—' }}</span>
            <span>标签</span><span>{{ item.tags?.length ? item.tags.join('、') : '—' }}</span>
            <span>分类</span><span>{{ item.categories?.length ? item.categories.join('、') : '—' }}</span>
            <span>文件夹</span><span>{{ item.folders?.[0] || '—' }}</span>
            <span>修改时间</span><span>{{ new Date(Number(item.modification_time)).toLocaleString() }}</span>
          </div>
          <div v-if="item.annotation" class="info-annotation">{{ item.annotation }}</div>
        </div>
      </template>
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

/* carousel：.swipe-track 只是视觉层（全屏定位承载轨道），手势统一落在 .gesture 上 */
.swipe-track {
  position: absolute;
  inset: 0;
  pointer-events: none;
}

/* 手势层：固定全屏承接 pointer 事件（命中测试与 pointer capture 始终有效）；
   位移由视觉层承载（transform 会改变元素命中区域，不能放在手势层上） */
.gesture {
  position: absolute;
  inset: 0;
  /* 触屏手势由 pointer 事件接管（滑动切换/捏合/拖拽平移），禁止浏览器默认触摸行为 */
  touch-action: none;
  cursor: grab;
}

.gesture.dragging {
  cursor: grabbing;
}

.track-row {
  display: flex;
  align-items: center;
  height: 100%;
  will-change: transform;
}

.track-img {
  flex: none;
  width: 100vw;
  height: 100%;
  object-fit: contain;
  /* 桌面保留 90vw/90vh 观感（网格区内边距），移动端由全局规则清零占满全屏 */
  padding: 5vh 5vw;
  box-sizing: border-box;
  user-select: none;
  pointer-events: none; /* 手势统一落在轨道上 */
  touch-action: none;
}

.track-slot {
  padding: 0;
}

.track-row.track-anim {
  transition: transform 0.17s ease;
}

.image {
  max-width: 90vw;
  max-height: 90vh;
  object-fit: contain;
  transform-origin: center;
  user-select: none;
  /* 纯视觉层：手势统一落在 .gesture 上 */
  pointer-events: none;
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

/* 移动端详情条：右上角 ⓘ 开关，底部滑出只读面板（close 在触屏隐藏，位置不与它冲突） */
.info-toggle {
  position: absolute;
  top: 12px;
  right: 16px;
  width: 32px;
  height: 32px;
  padding: 0;
  border-radius: 50%;
  border: 1px solid var(--border);
  background: rgba(30, 30, 30, 0.6);
  color: var(--fg-1);
  font-size: 16px;
  font-style: italic;
  font-family: Georgia, serif;
}

.info-toggle.open {
  color: var(--fg-0);
  border-color: var(--accent);
}

.info-sheet {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  max-height: 46vh;
  overflow-y: auto;
  /* 滑动手势不作用在条上，条内独立滚动（触屏纵向滚动与下拉关闭手势隔离） */
  touch-action: pan-y;
  padding: 14px 16px 20px;
  border-top: 1px solid var(--border);
  border-radius: 12px 12px 0 0;
  background: rgba(30, 30, 30, 0.92);
  backdrop-filter: blur(12px);
}

.info-name {
  font-size: 14px;
  font-weight: 600;
  word-break: break-all;
  margin-bottom: 10px;
}

.info-grid {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 6px 14px;
  font-size: 12px;
}

.info-grid > span:nth-child(odd) {
  color: var(--fg-1);
  white-space: nowrap;
}

.info-grid > span:nth-child(even) {
  word-break: break-all;
}

.info-annotation {
  margin-top: 10px;
  padding-top: 10px;
  border-top: 1px solid var(--border);
  font-size: 12px;
  color: var(--fg-1);
  white-space: pre-wrap;
  word-break: break-word;
}
</style>

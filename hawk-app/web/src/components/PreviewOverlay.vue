<script setup lang="ts">
import { computed, onMounted, watch } from 'vue';
import { useWindowSize } from '@vueuse/core';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
import { usePreviewStore } from '../stores/preview';
import { useContextMenu } from '../composables/useContextMenu';
import { useLayout } from '../composables/useLayout';
import { useZoomPan } from '../composables/useZoomPan';
import { itemKey, splitKey } from '../viewLogic';
import { isRotatableImage } from '../imageEdit';
import { saveImageToDisk } from '../saveImage';
import { showInFileManagerLabel, hasShell, shell } from '../platform';
import type { Item } from '../types';

const props = defineProps<{ item: Item }>();
const emit = defineEmits<{ close: []; navigate: [step: 1 | -1] }>();

const store = useLibraryStore();
const preview = usePreviewStore();
const menu = useContextMenu();
const { narrow, touch } = useLayout();

// 右上角关闭 ×：仅「触屏且窄屏」（手机）隐藏——下拉关闭替代；触屏宽屏（iPad 横屏/触屏笔记本）
// 鼠标没有下拉手势，且 touch 判定含 maxTouchPoints>0，混合设备不能没有关闭按钮。
// 设置面板「预览模式隐藏关闭按钮」开启时全端隐藏（Esc/双击/触屏下拉仍可关闭）。
const showClose = computed(() => (!touch.value || !narrow.value) && !preview.hidePreviewClose);

// 视口宽度（响应式）：carousel 轨道基准偏移 -100vw 依赖它；直接读 window.innerWidth 非响应式，
// 拖动窗口尺寸时图片 100vw 已变而 transform 仍是旧值，当前帧会漂移不居中
const { width: viewportW } = useWindowSize();

// 底部中间序号：当前项在视图中的位置 / 视图总条目数（Eagle 式）
const indexText = computed(() => {
  const i = preview.previewIndex;
  return i >= 0 ? `${i + 1} / ${store.total}` : '';
});

// 预览展示原图（缩略图是压缩过的 WebP）
const imageUrl = computed(() => api.fileUrl(props.item.id));

/** 复制图片本体到剪贴板：Web 标准 Clipboard API（Electron 44 已移除主进程 clipboard.writeImage，
 *  渲染进程的 navigator.clipboard 是全端可用路径）；原图经 item/file 拉取，按原 MIME 写入 */
async function copyImage() {
  try {
    const blob = await (await fetch(api.fileUrl(props.item.id))).blob();
    await navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })]);
    store.showToast('已复制图片');
  } catch {
    store.showToast('复制图片失败');
  }
}

// 右键/长按菜单：保存图片（仅浏览器端）/ 打开所在文件夹 / 复制文件路径 / 复制图片 / 编辑图片 / 删除。
// 浏览器（无 hawkShell）隐藏系统相关项；只读查看（viewer）隐藏全部写操作，无可用项时不弹菜单。
function onMenu(e: MouseEvent) {
  const items = [
    // 保存图片：仅浏览器端出现（桌面端文件本就在本机，走「在文件管理器中显示」）；
    // 读操作，viewer 只读也可用——移动端预览层是保存原图的主入口
    ...(!hasShell
      ? [
          {
            label: '保存图片',
            action: () =>
              void saveImageToDisk(props.item)
                .then((r) => {
                  if (r === 'saved') {
                    store.showToast('已保存图片');
                  }
                })
                .catch(() => store.showToast('保存图片失败')),
          },
        ]
      : []),
    ...(hasShell
      ? [
          { label: showInFileManagerLabel, action: () => void shell.showInFinder(props.item.paths[0]) },
          { label: '复制文件路径', action: () => void shell.copyPath(props.item.paths[0]) },
        ]
      : []),
    { label: '复制图片', action: () => void copyImage() },
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
  const key = itemKey(props.item.id, props.item.path);
  const fallback = preview.previewNavId(1) ?? preview.previewNavId(-1);
  store.select(key);
  await store.trashSelected();
  if (fallback && fallback !== key) {
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
  // 长按（触屏/笔）打开条目菜单：iOS 无原生长按菜单，移动端保存图片的入口
  onLongPress: (e) => onMenu(e),
});
const {
  scale, tx, ty, dragging,
  swipeAnim, swipeX,
  pullActive, pullAnim, pullY,
  onWheel, onPointerDown, onPointerMove, onPointerUp, onPointerCancel, onDblClick, onGestureClick,
} = gestures;

// 预加载相邻原图：内容寻址 immutable，浏览器缓存命中——carousel 拖动时邻图已解码，切换零等待
function preloadNeighbors() {
  for (const step of [1, -1] as const) {
    const key = preview.previewNavId(step);
    if (key) {
      new Image().src = api.fileUrl(splitKey(key).id);
    }
  }
}

onMounted(preloadNeighbors);

// carousel 邻居：key 取自骨架（不依赖详情窗口），取图按内容 id；拖动时左右邻图已经可见（iOS 相册式）
const prevKey = computed(() => preview.previewNavId(-1));
const nextKey = computed(() => preview.previewNavId(1));
const prevUrl = computed(() => (prevKey.value ? api.fileUrl(splitKey(prevKey.value).id) : null));
const nextUrl = computed(() => (nextKey.value ? api.fileUrl(splitKey(nextKey.value).id) : null));

// 缩放>1 的单图平移模式样式（carousel 模式由 trackStyle 负责）；手势层负责 cursor
const imageStyle = computed(() => ({
  transform: `translate(${tx.value}px, ${ty.value}px) scale(${scale.value})`,
}));

// carousel 轨道：三张并排（前|当前|后），基准 translateX=-100vw 使当前图居中，swipeX 为跟手偏移
// （基准取响应式 viewportW：拖动窗口时轨道随视口重算，当前帧保持居中）
const trackStyle = computed(() => ({
  transform: `translate(${swipeX.value - viewportW.value}px, ${pullActive.value || pullAnim.value ? pullY.value : 0}px)`,
}));

// 下拉跟手时背景随位移渐亮（松手回弹/滑出后恢复默认遮罩）
const overlayStyle = computed(() => {
  if (!pullActive.value && !pullAnim.value) {
    return {};
  }
  const dim = Math.max(0, 1 - pullY.value / 420);
  return { background: `rgba(0, 0, 0, ${(0.85 * dim).toFixed(3)})` };
});

watch(() => itemKey(props.item.id, props.item.path), () => {
  // 切图复位：手势状态机清零（手指通常已抬起，兜底防泄漏）+ 相邻预加载
  gestures.reset();
  preloadNeighbors();
});

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
          <img v-if="prevUrl" class="track-img" :src="prevUrl" :key="`p-${prevKey}`" alt="上一张" draggable="false" />
          <div v-else class="track-img track-slot" />
          <img class="track-img" :src="imageUrl" :key="item.id" :alt="item.name" draggable="false" />
          <img v-if="nextUrl" class="track-img" :src="nextUrl" :key="`n-${nextKey}`" alt="下一张" draggable="false" />
          <div v-else class="track-img track-slot" />
        </div>
      </div>
      <!-- 底部中间：序号 + 左右切换（Eagle 式） -->
      <div class="pager">
        <button class="page-btn" title="上一个 (←)" @click.stop="emit('navigate', -1)">‹</button>
        <span class="page-index">{{ indexText }}</span>
        <button class="page-btn" title="下一个 (→)" @click.stop="emit('navigate', 1)">›</button>
      </div>
      <button v-if="showClose" class="close" title="关闭 (Esc)" @click="emit('close')">×</button>
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
  width: 32px;
  border: none;
  background: transparent;
  color: var(--fg-1);
  font-size: 28px;
  text-align: center;
}

@media (hover: hover) {

.close:hover {
  color: #fff;
  background: transparent;
}
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

@media (hover: hover) {

.page-btn:hover {
  color: #fff;
  background: transparent;
}
}

.page-index {
  font-size: 13px;
  min-width: 64px;
  text-align: center;
  user-select: none;
}

</style>

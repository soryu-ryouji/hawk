<script setup lang="ts">
// 外观分区：缩略图尺寸滑杆（实时生效）+ 预览关闭按钮开关。无保存语义，改动即时写入偏好。
// 显隐由主组件 v-show 作用于本组件根元素。
import { useLibraryStore } from '../stores/library';
import { usePreviewStore } from '../stores/preview';

const store = useLibraryStore();
const preview = usePreviewStore();

/** 缩略图尺寸步进（滑杆 ± 按钮）：用户显式设置，写入偏好 */
function stepThumb(delta: number) {
  store.setUserThumbSize(store.thumbSize + delta);
}

/** 滑杆输入：用户显式设置（不复用动态默认的写入路径，避免被持久化逻辑混淆） */
function onThumbInput(e: Event) {
  store.setUserThumbSize(Number((e.target as HTMLInputElement).value));
}

/** 预览关闭按钮开关（即时生效）：写入偏好并记忆 */
function onHidePreviewClose(e: Event) {
  preview.setHidePreviewClose((e.target as HTMLInputElement).checked);
}
</script>

<template>
  <div class="pane">
    <div class="field">
      <span class="field-label">缩略图尺寸</span>
      <span class="slider-val">{{ store.thumbSize }}</span>
    </div>
    <div class="slider-row">
      <button title="缩小" @click="stepThumb(-8)">−</button>
      <input :value="store.thumbSize" type="range" min="120" max="280" step="8" @input="onThumbInput" />
      <button title="放大" @click="stepThumb(8)">＋</button>
    </div>

    <div class="switch-row">
      <div>
        <div class="switch-label">预览模式隐藏关闭按钮</div>
        <p class="hint">开启后全屏预览不显示右上角 ×；仍可用 Esc、双击或触屏下拉手势关闭。</p>
      </div>
      <label class="switch" title="预览模式隐藏关闭按钮">
        <input type="checkbox" :checked="preview.hidePreviewClose" @change="onHidePreviewClose" />
        <span class="track" />
      </label>
    </div>
  </div>
</template>

<style src="./settings-shared.css"></style>

<style scoped>
.slider-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.slider-row input[type='range'] {
  flex: 1;
  padding: 0;
  border: none;
  background: transparent;
}

.slider-val {
  margin-left: auto;
  color: var(--fg-1);
  font-variant-numeric: tabular-nums;
}
</style>

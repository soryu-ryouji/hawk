<script setup lang="ts">
// 外观分区：缩略图尺寸滑杆（实时生效）+ 预览关闭按钮开关 + 关窗行为（仅 Electron，实时生效）。
// 无保存语义，改动即时写入偏好。显隐由主组件 v-show 作用于本组件根元素。
import { onMounted, ref } from 'vue';
import { useLibraryStore } from '../stores/library';
import { usePreviewStore } from '../stores/preview';
import { hasShell, shell } from '../platform';
import SelectBox from './SelectBox.vue';
import type { CloseAction } from '../types';

const store = useLibraryStore();
const preview = usePreviewStore();

/** 关窗行为偏好（config.toml，主进程为唯一事实源）：打开面板时拉取，切换即写 */
const closeAction = ref<CloseAction>('exit');
const closeOptions: { value: CloseAction; label: string }[] = [
  { value: 'exit', label: '直接退出' },
  { value: 'tray', label: '关闭到托盘' },
];

onMounted(() => {
  void shell.getCloseAction().then((v) => {
    closeAction.value = v;
  });
});

function onCloseActionChange(action: CloseAction) {
  closeAction.value = action;
  void shell.setCloseAction(action);
}

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

    <div v-if="hasShell" class="field column">
      <span class="field-label">关闭行为</span>
      <SelectBox
        :model-value="closeAction"
        :options="closeOptions"
        @update:model-value="onCloseActionChange($event as CloseAction)"
      />
      <p class="hint">关闭到托盘：窗口驻留系统托盘、后台服务保持运行；从托盘菜单或再次启动唤起。</p>
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

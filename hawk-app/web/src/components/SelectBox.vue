<script setup lang="ts">
// 通用下拉选择。原生 select 的展开列表由 OS 绘制（Windows 上白底高亮蓝，与暗色主题脱节），
// 此处自绘触发框 + Teleport 浮层，视觉与 ContextMenu 一致（浮层底色/边框/阴影/勾选标记）。
// 浮层挂 body 且 fixed 定位：设置面板 .pane 有 overflow-y: auto，组件内部绝对定位会被裁剪。
import { computed, nextTick, onBeforeUnmount, ref } from 'vue';

const props = defineProps<{
  modelValue: string;
  options: { value: string; label: string }[];
  placeholder?: string;
  disabled?: boolean;
}>();
const emit = defineEmits<{ 'update:modelValue': [value: string] }>();

const open = ref(false);
const highlighted = ref(-1);
const triggerRef = ref<HTMLElement | null>(null);
const listRef = ref<HTMLElement | null>(null);
const pos = ref({ x: 0, y: 0, width: 0 });

const currentLabel = computed(
  () => props.options.find((o) => o.value === props.modelValue)?.label ?? '',
);

async function openList(): Promise<void> {
  if (props.disabled || open.value) {
    return;
  }
  const rect = triggerRef.value!.getBoundingClientRect();
  highlighted.value = props.options.findIndex((o) => o.value === props.modelValue);
  open.value = true;
  await nextTick();
  const list = listRef.value!;
  const gap = 4;
  // 下方放不下且上方空间足够时向上翻转
  const up =
    rect.bottom + gap + list.offsetHeight > window.innerHeight &&
    rect.top - gap - list.offsetHeight > 0;
  pos.value = {
    x: rect.left,
    y: up ? rect.top - gap - list.offsetHeight : rect.bottom + gap,
    width: rect.width,
  };
  list.querySelector('.option.selected')?.scrollIntoView({ block: 'nearest' });
  window.addEventListener('resize', close);
  window.addEventListener('scroll', onScroll, true);
  document.addEventListener('mousedown', onOutside, true);
}

function close(): void {
  if (!open.value) {
    return;
  }
  open.value = false;
  window.removeEventListener('resize', close);
  window.removeEventListener('scroll', onScroll, true);
  document.removeEventListener('mousedown', onOutside, true);
}

function toggle(): void {
  if (open.value) {
    close();
  } else {
    void openList();
  }
}

function choose(value: string): void {
  emit('update:modelValue', value);
  close();
}

/** 面板滚动时浮层不跟随，直接关闭（列表自身滚动除外：scroll 捕获阶段也会经过 window） */
function onScroll(event: Event): void {
  if (listRef.value?.contains(event.target as Node)) {
    return;
  }
  close();
}

function onOutside(event: Event): void {
  const target = event.target as Node;
  if (triggerRef.value?.contains(target) || listRef.value?.contains(target)) {
    return;
  }
  close();
}

function onArrow(dir: 1 | -1): void {
  if (!open.value) {
    void openList();
    return;
  }
  const n = props.options.length;
  if (!n) {
    return;
  }
  highlighted.value = (highlighted.value + dir + n) % n;
  void nextTick(() => {
    listRef.value
      ?.querySelectorAll('.option')
      [highlighted.value]?.scrollIntoView({ block: 'nearest' });
  });
}

/** Enter/Space：关闭态打开（prevent 掉了 button 默认 click 激活），打开态选中高亮项 */
function onConfirm(): void {
  if (!open.value) {
    void openList();
    return;
  }
  const opt = props.options[highlighted.value];
  if (opt) {
    choose(opt.value);
  }
}

onBeforeUnmount(() => {
  window.removeEventListener('resize', close);
  window.removeEventListener('scroll', onScroll, true);
  document.removeEventListener('mousedown', onOutside, true);
});
</script>

<template>
  <button
    ref="triggerRef"
    type="button"
    class="trigger"
    :class="{ open }"
    :disabled="disabled"
    @click="toggle"
    @keydown.down.prevent="onArrow(1)"
    @keydown.up.prevent="onArrow(-1)"
    @keydown.enter.prevent="onConfirm"
    @keydown.space.prevent="onConfirm"
    @keydown.esc.prevent.stop="close"
  >
    <span class="value" :class="{ placeholder: !currentLabel }">{{
      currentLabel || placeholder || '请选择'
    }}</span>
    <svg
      class="arrow"
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2.4"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="m6 9 6 6 6-6" />
    </svg>
  </button>
  <Teleport to="body">
    <div
      v-if="open"
      ref="listRef"
      class="list"
      :style="{ left: pos.x + 'px', top: pos.y + 'px', width: pos.width + 'px' }"
    >
      <button
        v-for="(opt, i) in options"
        :key="opt.value"
        type="button"
        class="option"
        :class="{ selected: opt.value === modelValue, active: i === highlighted }"
        @click="choose(opt.value)"
        @mouseenter="highlighted = i"
      >
        <span class="check">{{ opt.value === modelValue ? '✓' : '' }}</span>
        <span class="label">{{ opt.label }}</span>
      </button>
      <div v-if="!options.length" class="empty">无可选项</div>
    </div>
  </Teleport>
</template>

<style scoped>
.trigger {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 4px 10px;
  background: var(--bg-2);
  border: 1px solid var(--border);
  border-radius: 4px;
  color: var(--fg-0);
  text-align: left;
}

/* 输入类控件 hover 不变色（覆盖全局 button:hover），聚焦/展开才亮 accent 边框 */
@media (hover: hover) {
.trigger:hover {
  background: var(--bg-2);
}
}

.trigger:focus,
.trigger.open {
  border-color: var(--accent);
}

.trigger:disabled {
  opacity: 0.5;
  cursor: default;
}

.value {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.value.placeholder {
  color: var(--fg-1);
}

.arrow {
  flex: none;
  color: var(--fg-1);
  transition: transform 0.15s;
}

.trigger.open .arrow {
  transform: rotate(180deg);
}

.list {
  position: fixed;
  /* 设置对话框 170 / 预览浮层 200，须盖过两者 */
  z-index: 300;
  max-height: 240px;
  overflow-y: auto;
  padding: 4px;
  border-radius: 6px;
  background: var(--bg-3);
  border: 1px solid var(--border);
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.5);
  display: flex;
  flex-direction: column;
}

.option {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 6px 12px;
  border: none;
  border-radius: 4px;
  background: transparent;
  text-align: left;
}

@media (hover: hover) {
  .option:hover {
    background: color-mix(in srgb, var(--accent) 35%, transparent);
  }
}

/* 键盘高亮（↑↓ 导航）不是 hover 态，触屏外接键盘也要生效 */
.option.active {
  background: color-mix(in srgb, var(--accent) 35%, transparent);
}

.option .check {
  width: 12px;
  flex: none;
  color: var(--accent);
}

.option .label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.empty {
  padding: 6px 12px;
  font-size: 12px;
  color: var(--fg-1);
}
</style>

<script setup lang="ts">
// 8 位 hawk-app 配对码输入 / 展示：邮箱验证码式分格。输入层是一个透明原生 input，
// 键盘输入、粘贴、移动端一次一码提示都由系统原生处理；可见格子只负责展示与焦点提示。
import { computed, ref } from 'vue';

const props = withDefaults(
  defineProps<{
    modelValue: string;
    /** 码长；当前后端的 hawk-app 配对码固定 8 位 */
    length?: number;
    readonly?: boolean;
    label?: string;
  }>(),
  { length: 8, readonly: false, label: '配对码' },
);

const emit = defineEmits<{ 'update:modelValue': [value: string]; submit: [] }>();

const inputRef = ref<HTMLInputElement | null>(null);
const focused = ref(false);
const chars = computed(() => Array.from({ length: props.length }, (_, index) => props.modelValue[index] ?? ''));
/** 下一格高亮：码填满后不再显示空位焦点 */
const activeIndex = computed(() => (focused.value && !props.readonly && props.modelValue.length < props.length ? props.modelValue.length : -1));

function normalize(raw: string): string {
  return raw
    .toUpperCase()
    .replace(/[^A-Z0-9]/g, '')
    .slice(0, props.length);
}

function onInput(event: Event): void {
  const input = event.target as HTMLInputElement;
  const next = normalize(input.value);
  // 原生 input 受 v-model 外部值控制时不一定同步截断/过滤结果；就地回写避免 DOM 与 model 漂移
  if (input.value !== next) {
    input.value = next;
  }
  emit('update:modelValue', next);
}

function focus(): void {
  if (!props.readonly) {
    inputRef.value?.focus();
  }
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Enter') {
    event.preventDefault();
    emit('submit');
  }
}
</script>

<template>
  <div class="code-boxes" :class="{ readonly, focused }" :aria-label="label" @click="focus">
    <input
      v-if="!readonly"
      ref="inputRef"
      class="code-native"
      :value="modelValue"
      :maxlength="length"
      :aria-label="label"
      type="text"
      inputmode="text"
      autocomplete="one-time-code"
      autocapitalize="characters"
      spellcheck="false"
      @input="onInput"
      @focus="focused = true"
      @blur="focused = false"
      @keydown="onKeydown"
    />
    <span v-for="(char, index) in chars" :key="index" class="code-box" :class="{ filled: !!char, active: index === activeIndex }" aria-hidden="true">
      {{ char }}
    </span>
  </div>
</template>

<style scoped>
.code-boxes {
  position: relative;
  display: flex;
  gap: 5px;
  width: 100%;
  cursor: text;
}

.code-boxes.readonly {
  width: auto;
  cursor: default;
}

.code-native {
  position: absolute;
  inset: 0;
  z-index: 1;
  width: 100%;
  height: 100%;
  padding: 0;
  border: 0;
  opacity: 0;
  cursor: text;
}

.code-box {
  flex: 1;
  min-width: 0;
  height: 34px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--border);
  border-radius: 5px;
  background: var(--bg-1);
  color: var(--fg-0);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 14px;
  font-weight: 600;
  user-select: none;
}

.readonly .code-box {
  flex: none;
  width: 28px;
  height: 30px;
  font-size: 13px;
}

.code-box.filled {
  border-color: color-mix(in srgb, var(--fg-1) 45%, var(--border));
}

.code-box.active {
  border-color: var(--accent);
  box-shadow: inset 0 0 0 1px var(--accent);
}
</style>

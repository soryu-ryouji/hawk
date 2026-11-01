<script setup lang="ts">
import type { Directive } from 'vue';
import Icon from './Icon.vue';

withDefaults(
  defineProps<{
    title: string;
    description: string;
    confirmText?: string;
    cancelText?: string;
    danger?: boolean;
  }>(),
  { confirmText: '确定', cancelText: '取消', danger: false },
);
const emit = defineEmits<{ confirm: []; cancel: [] }>();

// 危险操作默认聚焦取消，Enter 不会误确认；按钮自身保留原生 Enter/Space 激活行为。
const vFocus: Directive<HTMLElement> = {
  mounted: (el) => el.focus(),
};
</script>

<template>
  <Teleport to="body">
    <div class="mask" @click.self="emit('cancel')">
      <div class="dialog" role="dialog" aria-modal="true" :aria-label="title" @keydown.stop @keydown.esc="emit('cancel')">
        <div class="heading">
          <span v-if="danger" class="warning-icon"><Icon name="trash" :size="17" /></span>
          <h2 class="title">{{ title }}</h2>
        </div>
        <p class="description">{{ description }}</p>
        <div class="actions">
          <button v-focus type="button" class="cancel" @click="emit('cancel')">{{ cancelText }}</button>
          <button type="button" :class="{ danger }" @click="emit('confirm')">{{ confirmText }}</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 300;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
  background: rgba(0, 0, 0, 0.58);
  backdrop-filter: blur(4px);
}

.dialog {
  width: min(400px, 100%);
  padding: 20px;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--bg-2);
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.4);
}

.heading {
  display: flex;
  align-items: center;
  gap: 10px;
}

.warning-icon {
  display: grid;
  place-items: center;
  width: 32px;
  height: 32px;
  flex: none;
  border-radius: 8px;
  background: color-mix(in srgb, var(--danger) 12%, transparent);
  color: var(--danger);
}

.title {
  color: var(--fg-0);
  font-size: 15px;
  font-weight: 600;
}

.description {
  margin-top: 14px;
  color: var(--fg-1);
  font-size: 13px;
  line-height: 1.7;
  user-select: text;
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 20px;
}

.actions button {
  min-width: 72px;
  padding: 6px 14px;
  border-radius: 6px;
}

.actions .cancel {
  background: var(--bg-1);
}

.actions .danger {
  border-color: var(--danger);
  background: var(--danger);
  color: #fff;
}
</style>

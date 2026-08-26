<script setup lang="ts">
import { ref } from 'vue';
import type { Directive } from 'vue';

defineProps<{ title: string; placeholder?: string }>();
const emit = defineEmits<{ confirm: [value: string]; cancel: [] }>();

const text = ref('');

const vFocus: Directive<HTMLElement> = {
  mounted: (el) => el.focus(),
};

function confirm() {
  const value = text.value.trim();
  if (value) {
    emit('confirm', value);
  } else {
    emit('cancel');
  }
}
</script>

<template>
  <Teleport to="body">
    <div class="mask" @click.self="emit('cancel')">
      <div class="dialog">
        <div class="title">{{ title }}</div>
        <input
          v-model="text"
          v-focus
          :placeholder="placeholder ?? ''"
          @keydown.enter="confirm"
          @keydown.esc="emit('cancel')"
        />
        <div class="actions">
          <button @click="emit('cancel')">取消</button>
          <button class="primary" @click="confirm">确定</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 150;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.5);
}

.dialog {
  width: 320px;
  padding: 16px;
  border-radius: 8px;
  background: var(--bg-2);
  border: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.title {
  font-weight: 600;
}

.dialog input {
  padding: 6px 8px;
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}
</style>

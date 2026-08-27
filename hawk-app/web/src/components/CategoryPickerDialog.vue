<script setup lang="ts">
import { ref } from 'vue';
import type { Directive } from 'vue';
import { useLibraryStore } from '../stores/library';

defineProps<{ title: string }>();
const emit = defineEmits<{ confirm: [path: string]; cancel: [] }>();

const store = useLibraryStore();
const text = ref('');

const vFocus: Directive<HTMLElement> = {
  mounted: (el) => el.focus(),
};

function confirm() {
  const path = text.value.trim().replace(/^\/+|\/+$/g, '');
  if (path) {
    emit('confirm', path);
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
          list="category-paths"
          placeholder="选择已有分类，或输入新路径（如 插画/人物）"
          @keydown.enter="confirm"
          @keydown.esc="emit('cancel')"
        />
        <datalist id="category-paths">
          <option v-for="category in store.flatCategories" :key="category.path" :value="category.path" />
        </datalist>
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
  width: 360px;
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

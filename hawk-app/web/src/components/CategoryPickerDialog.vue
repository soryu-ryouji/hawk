<script setup lang="ts">
import { ref } from 'vue';
import type { Directive } from 'vue';
import { useTaxonomyStore } from '../stores/taxonomy';

defineProps<{ title: string }>();
const emit = defineEmits<{ confirm: [name: string]; cancel: [] }>();

const taxonomy = useTaxonomyStore();
const text = ref('');

const vFocus: Directive<HTMLElement> = {
  mounted: (el) => el.focus(),
};

function confirm() {
  const name = text.value.trim();
  if (name) {
    emit('confirm', name);
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
          list="category-names"
          placeholder="选择已有分类，或输入新分类名称"
          @keydown.enter="confirm"
          @keydown.esc="emit('cancel')"
        />
        <datalist id="category-names">
          <option v-for="category in taxonomy.categories" :key="category.name" :value="category.name" />
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

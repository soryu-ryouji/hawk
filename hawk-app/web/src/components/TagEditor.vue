<script setup lang="ts">
import { ref } from 'vue';

const props = defineProps<{ modelValue: string[] }>();
const emit = defineEmits<{ 'update:modelValue': [value: string[]] }>();

const input = ref('');

function add() {
  const tag = input.value.trim();
  if (tag && !props.modelValue.includes(tag)) {
    emit('update:modelValue', [...props.modelValue, tag]);
  }
  input.value = '';
}

function remove(tag: string) {
  emit(
    'update:modelValue',
    props.modelValue.filter((t) => t !== tag),
  );
}
</script>

<template>
  <div class="tags">
    <span v-for="tag in modelValue" :key="tag" class="chip">
      {{ tag }}
      <button class="remove" title="移除标签" @click="remove(tag)">×</button>
    </span>
    <input v-model="input" class="input" placeholder="添加标签" @keydown.enter.prevent="add" @blur="add" />
  </div>
</template>

<style scoped>
.tags {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}

.chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 4px 2px 8px;
  border-radius: 10px;
  background: var(--bg-3);
  font-size: 12px;
}

.remove {
  padding: 0 4px;
  border: none;
  background: transparent;
  color: var(--fg-1);
}

.remove:hover {
  color: var(--danger);
  background: transparent;
}

.input {
  flex: 1;
  min-width: 80px;
  padding: 2px 6px;
  border-color: transparent;
  background: transparent;
}
</style>

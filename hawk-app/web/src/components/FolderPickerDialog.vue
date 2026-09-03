<script setup lang="ts">
import { ref } from 'vue';
import { useTaxonomyStore } from '../stores/taxonomy';

defineProps<{ title: string }>();
const emit = defineEmits<{ confirm: [path: string]; cancel: [] }>();

const taxonomy = useTaxonomyStore();
const selected = ref('');
</script>

<template>
  <Teleport to="body">
    <div class="mask" @click.self="emit('cancel')">
      <div class="dialog">
        <div class="title">{{ title }}</div>
        <select v-model="selected">
          <option v-for="folder in taxonomy.flatFolders" :key="folder.path" :value="folder.path">
            {{ folder.label }}
          </option>
        </select>
        <div class="actions">
          <button @click="emit('cancel')">取消</button>
          <button class="primary" @click="emit('confirm', selected)">确定</button>
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

.dialog select {
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

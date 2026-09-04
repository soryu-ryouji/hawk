<script setup lang="ts">
import { computed, ref } from 'vue';
import { useTaxonomyStore } from '../stores/taxonomy';
import SelectBox from './SelectBox.vue';

defineProps<{ title: string }>();
const emit = defineEmits<{ confirm: [path: string]; cancel: [] }>();

const taxonomy = useTaxonomyStore();
const selected = ref('');
const folderOptions = computed(() =>
  taxonomy.flatFolders.map((f) => ({ value: f.path, label: f.label })),
);
</script>

<template>
  <Teleport to="body">
    <div class="mask" @click.self="emit('cancel')">
      <div class="dialog">
        <div class="title">{{ title }}</div>
        <SelectBox v-model="selected" :options="folderOptions" placeholder="选择文件夹" />
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

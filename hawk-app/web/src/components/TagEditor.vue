<script setup lang="ts">
import { nextTick, ref } from 'vue';
import { useLibraryStore } from '../stores/library';

const props = defineProps<{ modelValue: string[] }>();
const emit = defineEmits<{ 'update:modelValue': [value: string[]] }>();

const store = useLibraryStore();

// 与分类/文件夹一致的「＋」模式：点击才展开输入框，Enter/失焦提交，Esc 取消
const editing = ref(false);
const input = ref('');
const inputEl = ref<HTMLInputElement>();

async function startEdit() {
  editing.value = true;
  await nextTick();
  inputEl.value?.focus();
}

function commit() {
  const tag = input.value.trim();
  if (tag && !props.modelValue.includes(tag)) {
    emit('update:modelValue', [...props.modelValue, tag]);
  }
  input.value = '';
  editing.value = false;
}

function cancel() {
  input.value = '';
  editing.value = false;
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
    <input
      v-if="editing"
      ref="inputEl"
      v-model="input"
      list="tag-suggestions"
      class="input"
      placeholder="标签名"
      @keydown.enter.prevent="commit"
      @keydown.esc.prevent="cancel"
      @blur="commit"
    />
    <button v-else class="add" title="新建标签" @click="startEdit">＋</button>
    <datalist id="tag-suggestions">
      <option v-for="t in store.tagList" :key="t.name" :value="t.name" />
    </datalist>
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

.add {
  padding: 0 8px;
  border-radius: 10px;
  font-size: 12px;
  line-height: 1.6;
}

.input {
  width: 110px;
  padding: 1px 6px;
  font-size: 12px;
}
</style>

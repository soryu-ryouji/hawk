<script setup lang="ts">
import { ref } from 'vue';
import type { Directive } from 'vue';

const props = defineProps<{
  title: string;
  placeholder?: string;
  suggestions?: string[];
  defaultValue?: string;
  /** 点击遮罩是否取消（默认 true）；预填内容的对话框传 false 防误触丢失输入 */
  dismissOnMask?: boolean;
}>();
const emit = defineEmits<{ confirm: [value: string]; cancel: [] }>();

// 预填默认值（重命名场景）：对话框每次条件渲染重建，取一次初值即可
const text = ref(props.defaultValue ?? '');
const listId = `dl-${Math.random().toString(36).slice(2)}`;

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

function onMaskClick(): void {
  if (props.dismissOnMask !== false) {
    emit('cancel');
  }
}
</script>

<template>
  <Teleport to="body">
    <div class="mask" @click.self="onMaskClick">
      <div class="dialog">
        <div class="title">{{ title }}</div>
        <input
          v-model="text"
          v-focus
          :list="suggestions?.length ? listId : undefined"
          :placeholder="placeholder ?? ''"
          @keydown.enter="confirm"
          @keydown.esc="emit('cancel')"
        />
        <datalist v-if="suggestions?.length" :id="listId">
          <option v-for="s in suggestions" :key="s" :value="s" />
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

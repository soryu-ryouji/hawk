<script setup lang="ts">
// 回收站专属顶栏操作：TitleBar 只负责按视图挂载，权限、确认与清空行为均封装在这里。
import { ref } from 'vue';
import { useLibraryStore } from '../store';
import { clearTrash } from '../actions';
import { useLayout } from '@/shared/composables/useLayout';
import Icon from '@/shared/ui/Icon.vue';
import ConfirmDialog from '@/shared/ui/ConfirmDialog.vue';

const store = useLibraryStore();
const { narrow } = useLayout();
const confirmOpen = ref(false);

function confirmClearTrash() {
  confirmOpen.value = false;
  void clearTrash();
}
</script>

<template>
  <button v-if="store.total > 0 && !store.editLocked" class="trash-clear" title="清空回收站" @click="confirmOpen = true">
    <Icon name="trash" :size="13" />
    <span>{{ narrow ? '清空' : '清空回收站' }}</span>
  </button>
  <ConfirmDialog
    v-if="confirmOpen"
    title="清空回收站？"
    :description="`回收站中的 ${store.total} 项素材将被彻底删除，此操作不可恢复。`"
    confirm-text="确认清空"
    danger
    @confirm="confirmClearTrash"
    @cancel="confirmOpen = false"
  />
</template>

<style scoped>
.trash-clear {
  /* 顶栏整体是窗口拖拽区；按钮自行退出，避免 Electron 拦截点击与 hover */
  -webkit-app-region: no-drag;
  display: flex;
  align-items: center;
  gap: 5px;
  flex: none;
  height: 26px;
  margin-left: 8px;
  padding: 0 8px;
  border-color: var(--border);
  background: transparent;
  color: var(--fg-1);
  font-size: 12px;
  white-space: nowrap;
}

@media (hover: hover) {
  .trash-clear:hover {
    border-color: var(--danger);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    color: var(--danger);
  }
}
</style>

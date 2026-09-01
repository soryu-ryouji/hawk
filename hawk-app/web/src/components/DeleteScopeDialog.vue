<script setup lang="ts">
// 多位置删除策略对话框：删除选中项里含多个库内位置副本的素材时触发
// （store.deleteScopePrompt 挂起的 resolve）。删除全部位置 = 卡片级删除（所有副本入回收站）；
// 仅从此处移除 = 只删当前文件夹视图范围内的位置（其余保留，卡片仍在其他文件夹可见）。
// 取消/Esc/点遮罩中止本次删除（什么都不删）。
import { computed } from 'vue';
import { useLibraryStore } from '../stores/library';

const store = useLibraryStore();
const prompt = computed(() => store.deleteScopePrompt);

function choose(choice: 'all' | 'folder' | 'cancel') {
  store.resolveDeleteScope(choice);
}
</script>

<template>
  <Teleport to="body">
    <div v-if="prompt" class="mask" @pointerdown.self="choose('cancel')">
      <div class="dialog" role="alertdialog" aria-modal="true" aria-label="删除多位置素材">
        <div class="title">删除多位置素材</div>
        <p class="message">
          {{ prompt.count }} 个选中素材存在于多个位置。
          <template v-if="prompt.folder"><br />「仅从此处移除」只删除「{{ prompt.folder }}」中的副本，其余位置保留。</template>
        </p>
        <div class="actions">
          <button @click="choose('cancel')">取消</button>
          <button v-if="prompt.folder" @click="choose('folder')">仅从此处移除</button>
          <button class="primary" @click="choose('all')">删除全部位置</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 180;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.5);
}

.dialog {
  width: min(380px, calc(100vw - 32px));
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

.message {
  margin: 0;
  color: var(--fg-1);
  font-size: 12px;
  line-height: 1.7;
}

.actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  flex-wrap: wrap;
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}
</style>

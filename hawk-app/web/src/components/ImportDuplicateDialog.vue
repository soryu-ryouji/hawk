<script setup lang="ts">
// 导入重复内容策略对话框：导入过程中首个「内容已在库内」的文件触发（store.dupPrompt 挂起
// 的 resolve），选择对整批生效。Esc/点遮罩按「忽略重复」处理（导入不中断）。
import { computed } from 'vue';
import { useLibraryStore } from '../stores/library';

const store = useLibraryStore();
const visible = computed(() => store.dupPrompt !== null);

function choose(choice: 'skip' | 'import') {
  store.resolveDuplicatePolicy(choice);
}
</script>

<template>
  <Teleport to="body">
    <div v-if="visible" class="mask" @pointerdown.self="choose('skip')">
      <div class="dialog" role="alertdialog" aria-modal="true" aria-label="发现重复内容">
        <div class="title">发现重复内容</div>
        <p class="message">
          部分待导入文件的内容已存在于素材库中。<br />
          忽略重复只跳过这些文件；仍然导入会为重复内容创建文件副本。
        </p>
        <div class="actions">
          <button class="primary" @click="choose('skip')">忽略重复</button>
          <button @click="choose('import')">仍然导入</button>
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
  width: min(360px, calc(100vw - 32px));
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
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}
</style>

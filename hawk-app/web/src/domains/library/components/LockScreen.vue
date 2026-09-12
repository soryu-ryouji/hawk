<script setup lang="ts">
import { computed, ref } from 'vue';
import type { Directive } from 'vue';
import { useLibraryStore } from '@/domains/library';
import { useTaxonomyStore } from '@/domains/taxonomy';
import Icon from '@/shared/ui/Icon.vue';

// 内容区锁占位界面：进入未解锁的锁定维度视图时替代网格（主 store lockedView 驱动）。
// 非模态：侧栏/标题栏照常可用，点别处即可离开；密码正确后解锁 → 原地重查 → 替换为实际内容
const store = useLibraryStore();
const taxonomy = useTaxonomyStore();

const password = ref('');
const busy = ref(false);

const dimLabel = computed(() => {
  const dim = store.lockedView?.dimension;
  return dim === 'folder' ? '文件夹' : dim === 'category' ? '分类' : '标签';
});
/** 锁条目名（文件夹可能是覆盖的祖先；显示最后一段即可辨识） */
const displayName = computed(() => store.lockedView?.name.split('/').pop() ?? '');

const vFocus: Directive<HTMLInputElement> = {
  mounted: (el) => el.focus(),
};

async function submit() {
  const target = store.lockedView;
  if (busy.value || !target || !password.value) {
    return;
  }
  busy.value = true;
  try {
    if (await taxonomy.unlock(target.dimension, target.name, password.value)) {
      password.value = ''; // 成功后界面整体退场，此处防御性清空
    }
  } finally {
    busy.value = false; // 失败保持输入（toast 已提示原因），可修改重试
  }
}
</script>

<template>
  <div class="lock-screen">
    <div class="panel">
      <Icon name="lock" :size="40" class="lock-icon" />
      <div class="title">此{{ dimLabel }}已锁定</div>
      <div class="target">{{ dimLabel }}「{{ displayName }}」的内容需要密码解锁后查看</div>
      <div class="input-row">
        <input v-model="password" v-focus type="password" placeholder="输入密码" autocomplete="off" @keydown.enter="submit" />
        <button class="primary" :disabled="busy || !password" @click="submit">解锁</button>
      </div>
      <div class="hint">解锁仅对当前客户端生效</div>
    </div>
  </div>
</template>

<style scoped>
.lock-screen {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
}

.panel {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  padding: 36px 44px;
  max-width: 420px;
}

.lock-icon {
  color: var(--fg-1);
  margin-bottom: 4px;
}

.title {
  font-size: 16px;
  font-weight: 600;
}

.target {
  color: var(--fg-1);
  font-size: 13px;
  word-break: break-all;
  text-align: center;
}

.input-row {
  display: flex;
  gap: 8px;
  margin-top: 10px;
  width: 100%;
}

.input-row input {
  flex: 1;
  padding: 7px 10px;
}

.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

.primary:disabled {
  opacity: 0.5;
}

.hint {
  color: var(--fg-1);
  font-size: 12px;
  opacity: 0.8;
}
</style>

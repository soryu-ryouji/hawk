<script setup lang="ts">
import { computed, ref } from 'vue';
import type { Directive } from 'vue';
import { useTaxonomyStore } from '@/domains/taxonomy';

// 锁管理对话框（右键菜单发起，admin）：set/change/remove 三模式。
// 解锁不走此组件——进入锁定视图时由内容区 LockScreen（主 store lockedView）承接。
const props = defineProps<{
  mode: 'set' | 'change' | 'remove';
  dimension: 'folder' | 'category' | 'tag';
  name: string;
}>();
const emit = defineEmits<{ close: [] }>();

const taxonomy = useTaxonomyStore();

const dimLabel = computed(() => (props.dimension === 'folder' ? '文件夹' : props.dimension === 'category' ? '分类' : '标签'));
const displayName = computed(() => (props.dimension === 'folder' ? props.name.split('/').pop() ?? props.name : props.name));

const title = computed(() => {
  switch (props.mode) {
    case 'set':
      return `锁定${dimLabel.value}`;
    case 'change':
      return `修改锁密码`;
    default:
      return `移除锁`;
  }
});
const primaryLabel = computed(() => {
  switch (props.mode) {
    case 'set':
      return '锁定';
    case 'change':
      return '修改';
    default:
      return '移除';
  }
});

const password = ref('');
const oldPassword = ref('');
const busy = ref(false);
const vFocus: Directive<HTMLInputElement> = {
  mounted: (el) => el.focus(),
};

async function submit() {
  if (busy.value || !password.value) {
    return;
  }
  busy.value = true;
  try {
    switch (props.mode) {
      case 'set':
        if (await taxonomy.lockSet(props.dimension, props.name, password.value)) {
          emit('close');
        }
        break;
      case 'change':
        if (await taxonomy.lockSet(props.dimension, props.name, password.value, oldPassword.value)) {
          emit('close');
        }
        break;
      case 'remove':
        if (await taxonomy.lockRemove(props.dimension, props.name, password.value)) {
          emit('close');
        }
        break;
    }
  } finally {
    busy.value = false; // 失败保持对话框打开（toast 已提示原因），可改密重试
  }
}
</script>

<template>
  <Teleport to="body">
    <div class="mask" @click.self="emit('close')">
      <div class="dialog">
        <div class="title">{{ title }}</div>
        <div class="target">{{ dimLabel }}「{{ displayName }}」</div>
        <p v-if="mode === 'set'" class="hint">设置密码后，该{{ dimLabel }}的内容需要解锁才能查看（列表、缩略图、原图均受保护）。</p>
        <input
          v-if="mode === 'change'"
          v-model="oldPassword"
          class="text-input"
          type="password"
          placeholder="旧密码"
          autocomplete="off"
        />
        <input
          v-model="password"
          v-focus
          class="text-input"
          type="password"
          :placeholder="mode === 'remove' ? '密码' : mode === 'change' ? '新密码' : '设置密码（请牢记）'"
          autocomplete="off"
          @keydown.enter="submit"
          @keydown.esc="emit('close')"
        />
        <div class="actions">
          <button @click="emit('close')">取消</button>
          <button class="primary" :disabled="busy || !password || (mode === 'change' && !oldPassword)" @click="submit">
            {{ primaryLabel }}
          </button>
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
  width: 340px;
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

.target {
  color: var(--fg-1);
  font-size: 13px;
  word-break: break-all;
}

.hint {
  margin: 0;
  color: var(--fg-1);
  font-size: 12px;
  line-height: 1.5;
}

.text-input {
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

.primary:disabled {
  opacity: 0.5;
}
</style>

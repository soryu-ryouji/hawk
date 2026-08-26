<script setup lang="ts">
import { ref } from 'vue';
import type { Directive } from 'vue';
import { useLibraryStore } from '../stores/library';
import { useContextMenu } from '../composables/useContextMenu';
import type { FolderNode } from '../types';

// 输入框自动聚焦指令（<script setup> 中以 vFocus 局部变量形式注册）
const vFocus: Directive<HTMLElement> = {
  mounted(el) {
    el.focus();
  },
};

const props = defineProps<{ node: FolderNode; depth: number }>();

const store = useLibraryStore();
const menu = useContextMenu();

const expanded = ref(props.depth < 1);
const editing = ref<false | 'rename' | 'create'>(false);
const editText = ref('');

function isActive() {
  return store.view.kind === 'folder' && store.view.path === props.node.path;
}

function startEdit(kind: 'rename' | 'create') {
  editText.value = kind === 'rename' ? props.node.name : '';
  editing.value = kind;
  expanded.value = true;
}

async function submitEdit() {
  const text = editText.value.trim();
  if (text) {
    if (editing.value === 'rename') {
      await store.folderRename(props.node.path, text);
    } else {
      await store.folderCreate(props.node.path, text);
    }
  }
  editing.value = false;
}

function onContextMenu(e: MouseEvent) {
  menu.open(
    [
      { label: '新建子文件夹', action: () => startEdit('create') },
      { label: '重命名', action: () => startEdit('rename') },
      { separator: true, label: '' },
      {
        label: '删除（移入回收站）',
        danger: true,
        action: () => {
          if (window.confirm(`删除文件夹「${props.node.name}」？其中素材将一并移入回收站。`)) {
            void store.folderDelete(props.node.path);
          }
        },
      },
    ],
    e,
  );
}
</script>

<template>
  <div>
    <div
      class="node"
      :class="{ active: isActive() }"
      :style="{ paddingLeft: 12 + depth * 14 + 'px' }"
      @click="store.setView({ kind: 'folder', path: node.path })"
      @contextmenu.prevent.stop="onContextMenu"
    >
      <span
        v-if="node.children.length > 0"
        class="arrow"
        :class="{ expanded }"
        @click.stop="expanded = !expanded"
        >▸</span
      >
      <span v-else class="arrow-placeholder" />
      <span v-if="editing !== 'rename'" class="name">{{ node.name }}</span>
      <input
        v-else
        v-model="editText"
        v-focus
        class="edit"
        @keydown.enter="submitEdit"
        @keydown.esc="editing = false"
        @blur="submitEdit"
        @click.stop
      />
    </div>

    <div v-if="editing === 'create'" class="node" :style="{ paddingLeft: 12 + (depth + 1) * 14 + 'px' }">
      <span class="arrow-placeholder" />
      <input
        v-model="editText"
        v-focus
        class="edit"
        placeholder="新文件夹名称"
        @keydown.enter="submitEdit"
        @keydown.esc="editing = false"
        @blur="submitEdit"
      />
    </div>

    <template v-if="expanded">
      <FolderTreeNode v-for="child in node.children" :key="child.path" :node="child" :depth="depth + 1" />
    </template>
  </div>
</template>

<style scoped>
.node {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 5px 8px 5px 0;
  cursor: pointer;
  overflow: hidden;
}

.node:hover {
  background: var(--bg-2);
}

.node.active {
  background: var(--accent);
  color: #fff;
}

.arrow {
  flex: none;
  width: 14px;
  color: var(--fg-1);
  transition: transform 0.1s;
}

.arrow.expanded {
  transform: rotate(90deg);
}

.arrow-placeholder {
  flex: none;
  width: 14px;
}

.name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.edit {
  flex: 1;
  min-width: 0;
  padding: 1px 6px;
}
</style>

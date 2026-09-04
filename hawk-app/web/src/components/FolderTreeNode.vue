<script setup lang="ts">
import { computed, ref } from 'vue';
import type { Directive } from 'vue';
import { useLibraryStore } from '../stores/library';
import { useTaxonomyStore } from '../stores/taxonomy';
import { useContextMenu } from '../composables/useContextMenu';
import { isItemsDrag, itemsDragOver, readItemsDrop } from '../dnd';
import type { FolderNode } from '../types';

// 输入框自动聚焦指令（<script setup> 中以 vFocus 局部变量形式注册）
const vFocus: Directive<HTMLElement> = {
  mounted(el) {
    el.focus();
  },
};

const props = defineProps<{ node: FolderNode; depth: number; filter?: string }>();

const store = useLibraryStore();
const taxonomy = useTaxonomyStore();
const menu = useContextMenu();

const expanded = ref(props.depth < 1);
const editing = ref<false | 'rename' | 'create'>(false);
const editText = ref('');

// ---- 侧栏筛选（底栏筛选框输入的关键词，调用方已转小写）：----
// 匹配节点与其祖先可见（祖先仅作上下文）；含匹配后代的节点强制展开，保证匹配链可达
function subtreeMatches(node: FolderNode): boolean {
  const kw = props.filter ?? '';
  return node.name.toLowerCase().includes(kw) || node.children.some(subtreeMatches);
}

const visible = computed(() => !props.filter || subtreeMatches(props.node));
const forcedExpand = computed(() => !!props.filter && props.node.children.some(subtreeMatches));
/** 展开箭头：筛选态下只有含匹配后代时才出现（否则点开是空链） */
const hasVisibleChildren = computed(() =>
  props.filter ? props.node.children.some(subtreeMatches) : props.node.children.length > 0,
);

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
      await taxonomy.folderRename(props.node.path, text);
    } else {
      await taxonomy.folderCreate(props.node.path, text);
    }
  }
  editing.value = false;
}

function onContextMenu(e: MouseEvent) {
  // 只读查看（局域网 viewer）：文件夹写操作入口隐藏
  if (store.viewerMode) {
    return;
  }
  menu.open(
    [
      { label: '新建子文件夹', action: () => startEdit('create') },
      { label: '重命名', action: () => startEdit('rename') },
      { separator: true, label: '' },
      { label: '刷新缓存', title: '修复该文件夹（含子目录）缺失的宽高/缩略图/调色板，并清除源文件已删除的残留条目', action: () => void store.refreshCache('folder', props.node.path, props.node.name) },
      { separator: true, label: '' },
      {
        label: '删除（移入回收站）',
        danger: true,
        action: () => {
          if (window.confirm(`删除文件夹「${props.node.name}」？其中素材将一并移入回收站。`)) {
            void taxonomy.folderDelete(props.node.path);
          }
        },
      },
    ],
    e,
  );
}

// ---- 素材拖入（网格 → 文件夹）：enter/leave 成对计数防子元素间闪烁 ----
const dropDepth = ref(0);

function onDragEnter(e: DragEvent) {
  if (isItemsDrag(e)) {
    dropDepth.value++;
  }
}

function onDragLeave() {
  dropDepth.value = Math.max(0, dropDepth.value - 1);
}

function onDragOver(e: DragEvent) {
  itemsDragOver(e);
}

function onDrop(e: DragEvent) {
  dropDepth.value = 0;
  if (readItemsDrop(e)) {
    store.moveSelectedToFolder(props.node.path);
  }
}
</script>

<template>
  <div v-if="visible">
    <div
      class="node"
      :class="{ active: isActive(), 'drop-target': dropDepth > 0 }"
      :style="{ paddingLeft: 12 + depth * 14 + 'px' }"
      @click="store.setView({ kind: 'folder', path: node.path })"
      @contextmenu.prevent.stop="onContextMenu"
      @dragenter="onDragEnter"
      @dragleave="onDragLeave"
      @dragover="onDragOver"
      @drop="onDrop"
    >
      <span
        v-if="hasVisibleChildren"
        class="arrow"
        :class="{ expanded: expanded || forcedExpand }"
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
      <span v-if="editing !== 'rename'" class="count">{{ node.count || '' }}</span>
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

    <template v-if="expanded || forcedExpand">
      <FolderTreeNode v-for="child in node.children" :key="child.path" :node="child" :depth="depth + 1" :filter="filter" />
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

@media (hover: hover) {

.node:hover {
  background: var(--bg-2);
}
}

.node.active {
  /* Eagle 式选中高亮:暗灰微亮(--bg-3),不用亮色 accent */
  background: var(--bg-3);
  color: #fff;
}

/* Eagle 式:选中行的计数纯白加粗,灰字压蓝底对比度不足 */
.node.active .count {
  color: #fff;
  font-weight: 600;
}

/* 素材悬停：整行高亮示意可放置 */
.node.drop-target {
  background: color-mix(in srgb, var(--accent) 30%, transparent);
  outline: 1px dashed var(--accent);
  outline-offset: -1px;
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
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.count {
  padding-right: 4px;
  font-size: 11px;
  color: var(--fg-1);
}


.edit {
  flex: 1;
  min-width: 0;
  padding: 1px 6px;
}
</style>

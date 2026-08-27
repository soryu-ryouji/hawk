<script setup lang="ts">
import { ref } from 'vue';
import { useLibraryStore } from '../stores/library';
import { useContextMenu } from '../composables/useContextMenu';
import Icon from './Icon.vue';
import FolderTreeNode from './FolderTreeNode.vue';
import CategoryTreeNode from './CategoryTreeNode.vue';
import PromptDialog from './PromptDialog.vue';

const store = useLibraryStore();
const menu = useContextMenu();
const hasShell = !!window.hawkShell;

const showCreateFolder = ref(false);
const showCreateCategory = ref(false);
const showCreateTag = ref(false);
const showRenameTag = ref(false);
const renameTarget = ref('');

async function selectLibrary() {
  if (window.hawkShell) {
    await window.hawkShell.selectLibrary();
  }
}

function createRootFolder(name: string) {
  showCreateFolder.value = false;
  void store.folderCreate('', name);
}

function createRootCategory(name: string) {
  showCreateCategory.value = false;
  void store.categoryCreate(name);
}

function createTag(name: string) {
  showCreateTag.value = false;
  void store.tagCreate(name);
}

function submitRenameTag(newName: string) {
  showRenameTag.value = false;
  void store.tagRename(renameTarget.value, newName);
}

/** 树空白处右键：新建根节点 */
function onTreeContextMenu(e: MouseEvent) {
  menu.open([{ label: '新建文件夹', action: () => (showCreateFolder.value = true) }], e);
}

function onCategoryContextMenu(e: MouseEvent) {
  menu.open([{ label: '新建分类', action: () => (showCreateCategory.value = true) }], e);
}

/** 标签右键：重命名/删除 */
function onTagContextMenu(name: string, e: MouseEvent) {
  menu.open(
    [
      {
        label: '重命名',
        action: () => {
          renameTarget.value = name;
          showRenameTag.value = true;
        },
      },
      {
        label: '删除标签',
        danger: true,
        action: () => {
          if (window.confirm(`删除标签「${name}」？全部素材的该标签将被清除。`)) {
            void store.tagDelete(name);
          }
        },
      },
    ],
    e,
  );
}
</script>

<template>
  <aside class="sidebar">
    <button v-if="hasShell" class="library-name" :title="store.library?.path + '（点击更换素材库）'" @click="selectLibrary">
      <Icon name="library" />
      <span class="lib-text">{{ store.library?.name ?? 'hawk' }}</span>
      <Icon name="chevronDown" :size="12" />
    </button>
    <div v-else class="library-name static">
      <Icon name="library" />
      <span class="lib-text">{{ store.library?.name ?? 'hawk' }}</span>
    </div>

    <div class="entry" :class="{ active: store.view.kind === 'all' }" @click="store.setView({ kind: 'all' })">
      <Icon name="all" />
      <span class="label">全部素材</span>
      <span class="count">{{ store.folders?.count ?? 0 }}</span>
    </div>

    <div class="section">
      <span>文件夹</span>
      <button class="add" title="新建文件夹" @click="showCreateFolder = true">＋</button>
    </div>
    <div class="tree" @contextmenu.prevent="onTreeContextMenu">
      <FolderTreeNode v-for="node in store.folders?.children ?? []" :key="node.path" :node="node" :depth="0" />
    </div>

    <div class="section">
      <span>分类</span>
      <button class="add" title="新建分类" @click="showCreateCategory = true">＋</button>
    </div>
    <div class="tree" @contextmenu.prevent="onCategoryContextMenu">
      <CategoryTreeNode v-for="node in store.categories?.children ?? []" :key="node.path" :node="node" :depth="0" />
    </div>

    <div class="section">
      <span>标签</span>
      <button class="add" title="新建标签" @click="showCreateTag = true">＋</button>
    </div>
    <div class="tags">
      <div
        v-for="tag in store.tagList"
        :key="tag.name"
        class="tag-row"
        :class="{ active: store.view.kind === 'tag' && store.view.name === tag.name }"
        @click="store.setView({ kind: 'tag', name: tag.name })"
        @contextmenu.prevent.stop="onTagContextMenu(tag.name, $event)"
      >
        <Icon name="tag" :size="13" />
        <span class="tag-name">{{ tag.name }}</span>
        <span class="tag-count">{{ tag.count }}</span>
      </div>
    </div>

    <div class="spacer" />

    <div class="entry trash" :class="{ active: store.view.kind === 'trash' }" @click="store.setView({ kind: 'trash' })">
      <Icon name="trash" />
      <span class="label">回收站</span>
      <span class="count">{{ store.trashTotal }}</span>
    </div>
  </aside>

  <PromptDialog v-if="showCreateFolder" title="新建文件夹" placeholder="文件夹名称" @confirm="createRootFolder" @cancel="showCreateFolder = false" />
  <PromptDialog v-if="showCreateCategory" title="新建分类" placeholder="分类路径（如 插画/人物）" @confirm="createRootCategory" @cancel="showCreateCategory = false" />
  <PromptDialog v-if="showCreateTag" title="新建标签" placeholder="标签名称" @confirm="createTag" @cancel="showCreateTag = false" />
  <PromptDialog v-if="showRenameTag" title="重命名标签" :placeholder="renameTarget" @confirm="submitRenameTag" @cancel="showRenameTag = false" />
</template>

<style scoped>
.sidebar {
  display: flex;
  flex-direction: column;
  background: var(--bg-1);
  border-right: 1px solid var(--border);
  overflow-y: auto;
  padding: 8px 0;
}

.library-name {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0 8px 8px;
  padding: 5px 6px;
  font-weight: 600;
  border: none;
  border-radius: 5px;
  background: transparent;
  text-align: left;
}

.library-name:hover {
  background: var(--bg-2);
}

.library-name.static {
  cursor: default;
}

.lib-text {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.section {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px 4px;
  color: var(--fg-1);
  font-size: 12px;
}

.add {
  padding: 0 6px;
  border: none;
  background: transparent;
  color: var(--fg-1);
  line-height: 1.2;
}

.add:hover {
  color: var(--accent);
  background: transparent;
}

.entry {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 6px 12px;
  cursor: pointer;
}

.entry:hover {
  background: var(--bg-2);
}

.entry.active {
  background: var(--accent);
  color: #fff;
}

.entry.active .count {
  color: rgba(255, 255, 255, 0.8);
}

.label {
  flex: 1;
}

.count {
  font-size: 11px;
  color: var(--fg-1);
}

.tag-row {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 4px 12px;
  cursor: pointer;
  overflow: hidden;
}

.tag-row:hover {
  background: var(--bg-2);
}

.tag-row.active {
  background: var(--accent);
  color: #fff;
}

.tag-row.active .tag-count {
  color: rgba(255, 255, 255, 0.8);
}

.tag-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tag-count {
  font-size: 11px;
  color: var(--fg-1);
}

.spacer {
  flex: 1;
}

.trash {
  border-top: 1px solid var(--border);
}
</style>

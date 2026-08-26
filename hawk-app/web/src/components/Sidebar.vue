<script setup lang="ts">
import { ref } from 'vue';
import { useLibraryStore } from '../stores/library';
import { useContextMenu } from '../composables/useContextMenu';
import FolderTreeNode from './FolderTreeNode.vue';
import PromptDialog from './PromptDialog.vue';

const store = useLibraryStore();
const menu = useContextMenu();
const hasShell = !!window.hawkShell;

const showCreateFolder = ref(false);

async function selectLibrary() {
  if (window.hawkShell) {
    await window.hawkShell.selectLibrary();
  }
}

function createRootFolder(name: string) {
  showCreateFolder.value = false;
  void store.folderCreate('', name);
}

/** 树空白处右键：新建根文件夹 */
function onTreeContextMenu(e: MouseEvent) {
  menu.open([{ label: '新建文件夹', action: () => (showCreateFolder.value = true) }], e);
}
</script>

<template>
  <aside class="sidebar">
    <div class="library-name" :title="store.library?.path">{{ store.library?.name ?? 'hawk' }}</div>

    <div
      class="entry"
      :class="{ active: store.view.kind === 'all' }"
      @click="store.setView({ kind: 'all' })"
    >
      全部素材
    </div>

    <div class="section">
      <span>文件夹</span>
      <button class="add-folder" title="新建文件夹" @click="showCreateFolder = true">＋</button>
    </div>
    <div class="tree" @contextmenu.prevent="onTreeContextMenu">
      <FolderTreeNode
        v-for="node in store.folders?.children ?? []"
        :key="node.path"
        :node="node"
        :depth="0"
      />
    </div>

    <div class="spacer" />

    <div
      class="entry trash"
      :class="{ active: store.view.kind === 'trash' }"
      @click="store.setView({ kind: 'trash' })"
    >
      回收站
    </div>

    <button v-if="hasShell" class="switch" @click="selectLibrary">更换素材库</button>

    <PromptDialog
      v-if="showCreateFolder"
      title="新建文件夹"
      placeholder="文件夹名称"
      @confirm="createRootFolder"
      @cancel="showCreateFolder = false"
    />
  </aside>
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
  padding: 4px 12px 12px;
  font-weight: 600;
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

.add-folder {
  padding: 0 6px;
  border: none;
  background: transparent;
  color: var(--fg-1);
  line-height: 1.2;
}

.add-folder:hover {
  color: var(--accent);
  background: transparent;
}

.entry {
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

.tree {
  flex: 0 1 auto;
}

.spacer {
  flex: 1;
}

.trash {
  border-top: 1px solid var(--border);
}

.switch {
  margin: 8px 12px 4px;
  color: var(--fg-1);
}
</style>

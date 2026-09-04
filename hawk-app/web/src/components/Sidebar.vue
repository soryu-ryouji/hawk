<script setup lang="ts">
import { computed, reactive, ref } from 'vue';
import { useLibraryStore } from '../stores/library';
import { useTaxonomyStore } from '../stores/taxonomy';
import { useContextMenu } from '../composables/useContextMenu';
import { hasShell, shell } from '../platform';
import { isItemsDrag, itemsDragOver, readItemsDrop } from '../dnd';
import Icon from './Icon.vue';
import FolderTreeNode from './FolderTreeNode.vue';
import LibraryDropdown from './LibraryDropdown.vue';
import PromptDialog from './PromptDialog.vue';
import TaxonomyRow from './TaxonomyRow.vue';
import type { FolderNode } from '../types';

const store = useLibraryStore();
const taxonomy = useTaxonomyStore();
const menu = useContextMenu();

// 三个分区的折叠态（点分区标题收起/展开，v-show 保留树节点内部的展开/编辑状态）
const collapsed = reactive({ folder: false, category: false, tag: false });

// ---- 底栏筛选框（Eagle 式）：按小写子串过滤文件夹树/分类/标签；树侧的祖先链处理在 FolderTreeNode ----
const navFilter = ref('');
const navKeyword = computed(() => navFilter.value.trim().toLowerCase());

const filteredCategories = computed(() =>
  navKeyword.value
    ? taxonomy.categories.filter((c) => c.name.toLowerCase().includes(navKeyword.value))
    : taxonomy.categories,
);
const filteredTags = computed(() =>
  navKeyword.value
    ? taxonomy.tagList.filter((t) => t.name.toLowerCase().includes(navKeyword.value))
    : taxonomy.tagList,
);

/** 文件夹树是否存在匹配（空态提示用；树节点的可见性判断在 FolderTreeNode 内） */
const hasFolderMatch = computed(() => {
  const walk = (nodes: FolderNode[]): boolean =>
    nodes.some((n) => n.name.toLowerCase().includes(navKeyword.value) || walk(n.children));
  return walk(taxonomy.folders?.children ?? []);
});
const noNavMatch = computed(
  () => !!navKeyword.value && !hasFolderMatch.value && !filteredCategories.value.length && !filteredTags.value.length,
);

/** 顶部拖拽条双击切换最大化（与 TitleBar 一致；条内无交互控件，无需排除判断） */
function onHeadDblClick() {
  void shell.toggleMaximizeWindow();
}

const showCreateFolder = ref(false);
const showCreateCategory = ref(false);
const showCreateTag = ref(false);
const showRenameTag = ref(false);
const renameTarget = ref('');
const showRenameCategory = ref(false);

function createRootFolder(name: string) {
  showCreateFolder.value = false;
  void taxonomy.folderCreate('', name);
}

function createRootCategory(name: string) {
  showCreateCategory.value = false;
  void taxonomy.categoryCreate(name);
}

function submitRenameCategory(newName: string) {
  showRenameCategory.value = false;
  void taxonomy.categoryRename(renameTarget.value, newName);
}

/** 分类行「重命名」：记住目标名并弹输入框（TaxonomyRow 发来） */
function startRenameCategory(name: string) {
  renameTarget.value = name;
  showRenameCategory.value = true;
}

function createTag(name: string) {
  showCreateTag.value = false;
  void taxonomy.tagCreate(name);
}

function submitRenameTag(newName: string) {
  showRenameTag.value = false;
  void taxonomy.tagRename(renameTarget.value, newName);
}

/** 标签行「重命名」：记住目标名并弹输入框（TaxonomyRow 发来） */
function startRenameTag(name: string) {
  renameTarget.value = name;
  showRenameTag.value = true;
}

/** 树空白处右键：新建根节点 / 整库刷新缓存 */
function onTreeContextMenu(e: MouseEvent) {
  menu.open(
    [
      { label: '新建文件夹', action: () => (showCreateFolder.value = true) },
      {
        label: '刷新缓存（整库）',
        title: '修复全部素材缺失的宽高/缩略图/调色板，并清除源文件已删除的残留条目',
        action: () => void store.refreshCache('library', undefined, '整库'),
      },
    ],
    e,
  );
}

// ---- 素材拖入（网格 → 分类/标签行）：容器级委托 + 单高亮键，enter/leave 计数防子元素间闪烁 ----
const dropDepth = reactive<Record<string, number>>({});
const dropKey = ref<string | null>(null);

type DropKind = 'category' | 'tag';

function rowKey(kind: DropKind, name: string) {
  return `${kind}:${name}`;
}

function onTreeDragEnter(kind: DropKind, e: DragEvent) {
  if (!isItemsDrag(e)) {
    return;
  }
  const row = (e.target as HTMLElement).closest('.tax-row');
  if (!row) {
    return;
  }
  const key = rowKey(kind, row.getAttribute('data-name') ?? '');
  dropDepth[key] = (dropDepth[key] ?? 0) + 1;
  dropKey.value = key;
}

function onTreeDragLeave(kind: DropKind, e: DragEvent) {
  const row = (e.target as HTMLElement).closest('.tax-row');
  if (!row) {
    return;
  }
  const key = rowKey(kind, row.getAttribute('data-name') ?? '');
  dropDepth[key] = Math.max(0, (dropDepth[key] ?? 0) - 1);
  if ((dropDepth[key] ?? 0) === 0 && dropKey.value === key) {
    dropKey.value = null;
  }
}

function onTreeDragOver(e: DragEvent) {
  // 只读查看（局域网 viewer）：拖拽移动素材即写操作,不允许放置
  if (store.viewerMode) {
    return;
  }
  itemsDragOver(e);
}

function onTreeDrop(kind: DropKind, e: DragEvent) {
  dropKey.value = null;
  for (const k of Object.keys(dropDepth)) {
    dropDepth[k] = 0;
  }
  const row = (e.target as HTMLElement).closest('.tax-row');
  if (!row || !readItemsDrop(e)) {
    return;
  }
  const name = row.getAttribute('data-name');
  if (!name) {
    return;
  }
  if (kind === 'category') {
    void store.addCategoryToSelected(name);
  } else {
    void store.addTagToSelected(name);
  }
}

function onCategoryContextMenu(e: MouseEvent) {
  if (store.viewerMode) {
    return;
  }
  menu.open([{ label: '新建分类', action: () => (showCreateCategory.value = true) }], e);
}
</script>

<template>
  <aside class="sidebar">
    <!-- 顶部拖拽条：侧栏色块通高到窗口上沿；macOS 原生红绿灯压在本条左侧；右端为侧栏开关 -->
    <div class="sidebar-head" @dblclick="onHeadDblClick">
      <!-- 触屏：库名上移到本条与开关同排，正文整体上移填充空位；桌面/macOS 保持库名在正文首行（避让红绿灯） -->
      <LibraryDropdown v-if="hasShell" class="in-head" />
      <div v-else class="library-name in-head static">
        <Icon name="library" />
        <span class="lib-text">{{ store.library?.name ?? 'hawk' }}</span>
      </div>
      <button class="panel-toggle" title="侧栏与检查器" @click="store.toggleSidebar()" @dblclick.stop>
        <Icon name="panelLeft" :size="16" />
      </button>
    </div>
    <div class="sidebar-body">
      <LibraryDropdown v-if="hasShell" class="in-body" />
      <div v-else class="library-name in-body static">
        <Icon name="library" />
        <span class="lib-text">{{ store.library?.name ?? 'hawk' }}</span>
      </div>

      <div class="entry" :class="{ active: store.view.kind === 'all' }" @click="store.setView({ kind: 'all' })">
        <Icon name="all" />
        <span class="label">全部素材</span>
        <span class="count">{{ taxonomy.folders?.count ?? 0 }}</span>
      </div>

      <div class="entry" :class="{ active: store.view.kind === 'root' }" @click="store.setView({ kind: 'root' })">
        <Icon name="home" />
        <span class="label">根目录素材</span>
        <span class="count">{{ taxonomy.rootCount }}</span>
      </div>

      <div class="entry" :class="{ active: store.view.kind === 'uncategorized' }" @click="store.setView({ kind: 'uncategorized' })">
        <Icon name="inbox" />
        <span class="label">未分类素材</span>
        <span class="count">{{ taxonomy.uncategorizedCount }}</span>
      </div>

      <div class="entry" :class="{ active: store.view.kind === 'untagged' }" @click="store.setView({ kind: 'untagged' })">
        <Icon name="tagOff" />
        <span class="label">未标签素材</span>
        <span class="count">{{ taxonomy.untaggedCount }}</span>
      </div>

      <div class="entry" :class="{ active: store.view.kind === 'trash' }" @click="store.setView({ kind: 'trash' })">
        <Icon name="trash" />
        <span class="label">回收站</span>
        <span class="count">{{ taxonomy.trashTotal }}</span>
      </div>

      <div class="section" @click="collapsed.folder = !collapsed.folder">
        <span class="section-title">
          <Icon name="chevronRight" :size="12" class="chev" :class="{ open: !collapsed.folder }" />
          文件夹
        </span>
        <button v-if="!store.viewerMode" class="add" title="新建文件夹" @click.stop="showCreateFolder = true">＋</button>
      </div>
      <div v-show="!collapsed.folder" class="tree" @contextmenu.prevent="onTreeContextMenu">
        <FolderTreeNode v-for="node in taxonomy.folders?.children ?? []" :key="node.path" :node="node" :depth="0" :filter="navKeyword" />
      </div>

      <div class="section" @click="collapsed.category = !collapsed.category">
        <span class="section-title">
          <Icon name="chevronRight" :size="12" class="chev" :class="{ open: !collapsed.category }" />
          分类
        </span>
        <button v-if="!store.viewerMode" class="add" title="新建分类" @click.stop="showCreateCategory = true">＋</button>
      </div>
      <div
        v-show="!collapsed.category"
        class="tree"
        @contextmenu.prevent="onCategoryContextMenu"
        @dragenter="onTreeDragEnter('category', $event)"
        @dragleave="onTreeDragLeave('category', $event)"
        @dragover="onTreeDragOver"
        @drop="onTreeDrop('category', $event)"
      >
        <TaxonomyRow
          v-for="category in filteredCategories"
          :key="category.name"
          kind="category"
          :name="category.name"
          :count="category.count"
          :active="store.view.kind === 'category' && store.view.name === category.name"
          :drop-target="dropKey === rowKey('category', category.name)"
          @rename="startRenameCategory"
        />
      </div>

      <div class="section" @click="collapsed.tag = !collapsed.tag">
        <span class="section-title">
          <Icon name="chevronRight" :size="12" class="chev" :class="{ open: !collapsed.tag }" />
          标签
        </span>
        <button v-if="!store.viewerMode" class="add" title="新建标签" @click.stop="showCreateTag = true">＋</button>
      </div>
      <div
        v-show="!collapsed.tag"
        class="tags"
        @dragenter="onTreeDragEnter('tag', $event)"
        @dragleave="onTreeDragLeave('tag', $event)"
        @dragover="onTreeDragOver"
        @drop="onTreeDrop('tag', $event)"
      >
        <TaxonomyRow
          v-for="tag in filteredTags"
          :key="tag.name"
          kind="tag"
          :name="tag.name"
          :count="tag.count"
          :active="store.view.kind === 'tag' && store.view.name === tag.name"
          :drop-target="dropKey === rowKey('tag', tag.name)"
          @rename="startRenameTag"
        />
      </div>

      <div v-if="noNavMatch" class="nav-filter-empty">无匹配项</div>
    </div>

    <!-- 底栏筛选框：固定在侧栏底部，不随分区列表滚动；清空沿用 type=search 的原生 ×（与顶栏搜索框一致） -->
    <div class="nav-filter">
      <Icon name="filter" :size="12" />
      <input v-model="navFilter" type="search" placeholder="筛选" />
    </div>
  </aside>

  <PromptDialog v-if="showCreateFolder" title="新建文件夹" placeholder="文件夹名称" @confirm="createRootFolder" @cancel="showCreateFolder = false" />
  <PromptDialog v-if="showCreateCategory" title="新建分类" placeholder="分类名称" @confirm="createRootCategory" @cancel="showCreateCategory = false" />
  <PromptDialog v-if="showRenameCategory" title="重命名分类" :placeholder="renameTarget" @confirm="submitRenameCategory" @cancel="showRenameCategory = false" />
  <PromptDialog v-if="showCreateTag" title="新建标签" placeholder="标签名称" @confirm="createTag" @cancel="showCreateTag = false" />
  <PromptDialog v-if="showRenameTag" title="重命名标签" :placeholder="renameTarget" @confirm="submitRenameTag" @cancel="showRenameTag = false" />
</template>

<style scoped>
.sidebar {
  display: flex;
  flex-direction: column;
  background: var(--bg-1);
  border-right: 1px solid var(--border);
  overflow: hidden;
}

.sidebar-head {
  flex: none;
  height: 40px;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  padding: 0 8px;
  -webkit-app-region: drag;
}

/* 侧栏开关：可见时在本条右端（激活态）；条在拖拽区内，按钮须退出拖拽 */
.panel-toggle {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  padding: 0;
  border: none;
  border-radius: 5px;
  background: transparent;
  color: var(--accent);
  -webkit-app-region: no-drag;
}

.panel-toggle:hover {
  background: var(--bg-3);
}

.sidebar-body {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  padding-bottom: 8px;
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
  /* 顶条内实例（触屏）须退出拖拽区 */
  -webkit-app-region: no-drag;
}

/* 顶条内实例默认隐藏；触屏显示并与开关同排（flex:1 吃掉中段空白，名称超长省略） */
.library-name.in-head {
  display: none;
}

body.touch .library-name.in-head {
  display: flex;
  flex: 1;
  min-width: 0;
  margin: 0 4px 0 8px;
  padding: 4px 6px;
}

body.touch .library-name.in-body {
  display: none;
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
  cursor: pointer;
}

.section-title {
  display: flex;
  align-items: center;
  gap: 3px;
}

.chev {
  transition: transform 0.1s;
}

.chev.open {
  transform: rotate(90deg);
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
  /* Eagle 式选中高亮:暗灰微亮(--bg-3),不用亮色 accent */
  background: var(--bg-3);
  color: #fff;
}

.entry.active .count {
  color: #fff;
  font-weight: 600;
}

.label {
  flex: 1;
}

.count {
  font-size: 11px;
  color: var(--fg-1);
}

/* ---- 底栏筛选框：Eagle 式，固定在侧栏底部 ---- */
.nav-filter {
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 4px 8px 8px;
  padding: 0 8px;
  height: 28px;
  border: 1px solid var(--border);
  border-radius: 5px;
  background: var(--bg-2);
  color: var(--fg-1);
}

.nav-filter:focus-within {
  border-color: var(--accent);
}

.nav-filter input {
  flex: 1;
  min-width: 0;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--fg-0);
  font-size: 12px;
}

/* 保持输入框在聚焦时不重复 accent 边框（全局 input:focus 规则） */
.nav-filter input:focus {
  border-color: transparent;
}

.nav-filter-empty {
  padding: 12px;
  font-size: 12px;
  color: var(--fg-1);
  text-align: center;
}

</style>

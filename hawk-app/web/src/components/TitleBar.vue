<script setup lang="ts">
// Eagle 式中栏顶栏（只覆盖内容区，左右栏通高）：侧栏开关 · 前进/后退 · 位置面包屑 ‖ 缩略图滑杆 ‖ 排序/筛选 · 搜索。
// 整条为窗口拖拽区域（双击空白切换最大化），交互控件单独 no-drag。
import { computed, ref } from 'vue';
import { useLibraryStore } from '../stores/library';
import { useContextMenu } from '../composables/useContextMenu';
import Icon from './Icon.vue';
import type { QueryState } from '../types';

const store = useLibraryStore();
const { open: openMenu } = useContextMenu();
const emit = defineEmits<{ 'open-settings': [] }>();
const searchText = ref('');
const isMac = window.hawkShell?.platform === 'darwin';
/** 桌面端（Electron）才有设置面板入口 */
const hasShell = !!window.hawkShell;
// 窗口控制为 fixed 在窗口右上角的自绘按钮（仅 Windows/Linux 渲染）：侧栏隐藏时本栏通栏，右端需预留避让
const reserveControls = !!window.hawkShell && !isMac;

/** 文件夹视图显示可点击面包屑（根 = 全部素材），其余视图显示固定标题 */
const breadcrumb = computed(() => {
  const view = store.view;
  if (view.kind !== 'folder') {
    return null;
  }
  const segs = view.path.split('/');
  return {
    kind: view.kind,
    segs: segs.map((name, i) => ({ name, path: segs.slice(0, i + 1).join('/') })),
  };
});

const locationTitle = computed(() => {
  const view = store.view;
  if (view.kind === 'all') return '全部素材';
  if (view.kind === 'root') return '根目录素材';
  if (view.kind === 'uncategorized') return '未分类素材';
  if (view.kind === 'untagged') return '未标签素材';
  if (view.kind === 'trash') return '回收站';
  if (view.kind === 'tag' || view.kind === 'category') return view.name;
  return '';
});

/** 排序选项（字段 × 方向，弹层单选；不再用顶栏大下拉） */
const SORT_OPTIONS: { label: string; orderBy: QueryState['orderBy']; order: 'asc' | 'desc' }[] = [
  { label: '修改时间 ↓', orderBy: 'modification_time', order: 'desc' },
  { label: '修改时间 ↑', orderBy: 'modification_time', order: 'asc' },
  { label: '名称 ↑', orderBy: 'name', order: 'asc' },
  { label: '名称 ↓', orderBy: 'name', order: 'desc' },
  { label: '大小 ↓', orderBy: 'size', order: 'desc' },
  { label: '大小 ↑', orderBy: 'size', order: 'asc' },
  { label: '评分 ↓', orderBy: 'star', order: 'desc' },
  { label: '评分 ↑', orderBy: 'star', order: 'asc' },
];

const currentSortLabel = computed(
  () => SORT_OPTIONS.find((o) => o.orderBy === store.query.orderBy && o.order === store.query.order)?.label ?? '',
);

/** 排序按钮：弹出二级菜单设置字段与方向（当前项打勾） */
function openSortMenu(e: MouseEvent) {
  openMenu(
    SORT_OPTIONS.map((o) => ({
      label: o.label,
      checked: o.orderBy === store.query.orderBy && o.order === store.query.order,
      action: () => store.setQuery({ orderBy: o.orderBy, order: o.order }),
    })),
    e,
  );
}

function submitSearch() {
  store.setQuery({ keywords: searchText.value.trim().split(/\s+/).filter(Boolean) });
}

function stepThumb(delta: number) {
  store.thumbSize = Math.min(280, Math.max(120, store.thumbSize + delta));
}

/** 双击标题栏空白区切换最大化；点在控件上不触发 */
function onDblClick(e: MouseEvent) {
  if ((e.target as HTMLElement).closest('button, input, select, .search-box')) {
    return;
  }
  void window.hawkShell?.toggleMaximizeWindow();
}
</script>

<template>
  <!-- 侧栏隐藏时本栏通栏：macOS 左端避让原生红绿灯，Windows/Linux 右端避让自绘窗口控制 -->
  <header
    class="titlebar"
    :class="{ 'reserve-traffic': isMac && !store.sidebarVisible, 'reserve-controls': reserveControls && !store.sidebarVisible }"
    @dblclick="onDblClick"
  >
    <div class="group left">
      <!-- 侧栏可见时开关在侧栏顶条右端；隐藏时挪到本栏左上角 -->
      <button v-if="!store.sidebarVisible" class="bar-btn" title="侧栏与检查器" @click="store.toggleSidebar()">
        <Icon name="panelLeft" :size="16" />
      </button>
      <button class="bar-btn" title="后退" :disabled="!store.canGoBack" @click="store.goBack()">
        <Icon name="chevronLeft" :size="16" />
      </button>
      <button class="bar-btn" title="前进" :disabled="!store.canGoForward" @click="store.goForward()">
        <Icon name="chevronRight" :size="16" />
      </button>

      <nav v-if="breadcrumb" class="location crumbs">
        <button class="crumb" @click="store.setView({ kind: 'all' })">全部素材</button>
        <template v-for="(seg, i) in breadcrumb.segs" :key="seg.path">
          <Icon name="chevronRight" :size="12" class="sep" />
          <button
            class="crumb"
            :class="{ current: i === breadcrumb.segs.length - 1 }"
            @click="store.setView({ kind: breadcrumb.kind, path: seg.path })"
          >
            {{ seg.name }}
          </button>
        </template>
      </nav>
      <span v-else class="location title">{{ locationTitle }}</span>

      <span v-if="store.selection.length" class="selected-count">已选 {{ store.selection.length }} 项</span>
    </div>

    <div class="spacer" />

    <div class="group center">
      <button class="bar-btn" title="缩小缩略图" @click="stepThumb(-16)"><Icon name="minus" :size="13" /></button>
      <input v-model.number="store.thumbSize" class="thumb-slider" type="range" min="120" max="280" step="8" title="缩略图尺寸" />
      <button class="bar-btn" title="放大缩略图" @click="stepThumb(16)"><Icon name="plus" :size="13" /></button>
    </div>

    <div class="spacer" />

    <div class="group right">
      <button
        class="bar-btn filter-btn"
        :class="{ active: store.filterBarVisible || store.hasActiveFilters }"
        title="筛选工具列"
        @click="store.toggleFilterBar()"
      >
        <Icon name="filter" :size="14" />
      </button>

      <button class="bar-btn sort-btn" :title="`排序：${currentSortLabel}`" @click="openSortMenu">
        <Icon name="arrowUpDown" :size="14" />
      </button>

      <div class="search-box">
        <Icon name="search" :size="13" />
        <input v-model="searchText" type="search" placeholder="搜索" @keydown.enter="submitSearch" />
      </div>

      <button v-if="hasShell" class="bar-btn" title="设置" @click="emit('open-settings')">
        <Icon name="settings" :size="14" />
      </button>
    </div>
  </header>
</template>

<style scoped>
.titlebar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 10px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-1);
  -webkit-app-region: drag;
}

/* 交互控件退出拖拽区域 */
.titlebar button,
.titlebar input,
.titlebar select,
.titlebar .search-box {
  -webkit-app-region: no-drag;
}

.group {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
}

/* 侧栏隐藏时标题栏通栏：macOS 左端避开窗口左上角的原生红绿灯 */
.titlebar.reserve-traffic {
  padding-left: 78px;
}

/* 侧栏隐藏时标题栏通栏：右端避开 fixed 在窗口右上角的自绘窗口控制（3 × 42px + 间隙） */
.titlebar.reserve-controls {
  padding-right: 130px;
}

.group.right {
  gap: 8px;
}

.spacer {
  flex: 1;
}

.bar-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  padding: 0;
  border: none;
  border-radius: 5px;
  background: transparent;
  color: var(--fg-1);
}

.bar-btn:hover {
  background: var(--bg-3);
  color: var(--fg-0);
}

.bar-btn:disabled {
  opacity: 0.35;
  background: transparent;
  color: var(--fg-1);
  cursor: default;
}

.bar-btn.active {
  color: var(--accent);
}

.location {
  margin-left: 4px;
  font-size: 13px;
  white-space: nowrap;
}

.title {
  font-weight: 600;
}

.crumbs {
  display: flex;
  align-items: center;
  gap: 2px;
  overflow: hidden;
}

.crumb {
  padding: 2px 6px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--fg-1);
  white-space: nowrap;
}

.crumb:hover {
  background: var(--bg-3);
  color: var(--fg-0);
}

.crumb.current {
  color: var(--fg-0);
  font-weight: 600;
}

.sep {
  color: var(--fg-1);
}

.selected-count {
  color: var(--fg-1);
  font-size: 12px;
  white-space: nowrap;
}

.center {
  gap: 2px;
}

.thumb-slider {
  width: 140px;
  padding: 0;
  border: none;
  background: transparent;
}

.search-box {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
  border: 1px solid var(--border);
  border-radius: 5px;
  background: var(--bg-2);
  color: var(--fg-1);
  align-self: center;
}

.search-box input {
  width: 160px;
  padding: 4px 0;
  border: none;
  background: transparent;
}
</style>

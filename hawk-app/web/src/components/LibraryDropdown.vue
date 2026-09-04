<script setup lang="ts">
// 侧栏「素材库」下拉：库名触发按钮 + 下方对齐展开的历史库浮层（下拉模式，非右键菜单）。
// 浮层 Teleport 到 body 且 fixed 定位：侧栏有 overflow-y: auto，组件内绝对定位会被裁剪；
// 视觉与 SelectBox/ContextMenu 统一（底色/边框/阴影/勾选标记）。
// 历史由主进程记录（最近在前），当前库打勾且不可移除；目录已删的置灰不可切换但可移除记录。
// 换库就绪经 hawk:server-started 事件驱动 App 原地重启数据。
import { nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { shell } from '../platform';
import { useLibraryStore } from '../stores/library';
import Icon from './Icon.vue';
import type { LibraryHistoryItem } from '../types';

const store = useLibraryStore();

const open = ref(false);
const current = ref<string | null>(null);
const libraries = ref<LibraryHistoryItem[]>([]);
const triggerRef = ref<HTMLElement | null>(null);
const panelRef = ref<HTMLElement | null>(null);
const pos = ref({ x: 0, y: 0, width: 0 });

watch(open, async (visible) => {
  if (!visible) {
    window.removeEventListener('resize', close);
    window.removeEventListener('scroll', onScroll, true);
    document.removeEventListener('mousedown', onOutside, true);
    document.removeEventListener('keydown', onKeydown, true);
    return;
  }
  const res = await shell.listLibraries();
  current.value = res.current;
  libraries.value = res.libraries;
  const rect = triggerRef.value!.getBoundingClientRect();
  await nextTick();
  const panel = panelRef.value!;
  const gap = 4;
  // 下方放不下且上方空间足够时向上翻转
  const up =
    rect.bottom + gap + panel.offsetHeight > window.innerHeight &&
    rect.top - gap - panel.offsetHeight > 0;
  pos.value = {
    x: rect.left,
    y: up ? rect.top - gap - panel.offsetHeight : rect.bottom + gap,
    width: rect.width,
  };
  window.addEventListener('resize', close);
  window.addEventListener('scroll', onScroll, true);
  document.addEventListener('mousedown', onOutside, true);
  document.addEventListener('keydown', onKeydown, true);
});

function close(): void {
  open.value = false;
}

/** 侧栏滚动时浮层不跟随，直接关闭（浮层自身滚动除外：scroll 捕获阶段也会经过 window） */
function onScroll(event: Event): void {
  if (panelRef.value?.contains(event.target as Node)) {
    return;
  }
  close();
}

function onOutside(event: Event): void {
  const target = event.target as Node;
  if (triggerRef.value?.contains(target) || panelRef.value?.contains(target)) {
    return;
  }
  close();
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    event.stopPropagation();
    close();
  }
}

/** 切换素材库：仅对存在的非当前库生效（已删除项置灰） */
function switchTo(lib: LibraryHistoryItem): void {
  if (!lib.exists || lib.path === current.value) {
    return;
  }
  close();
  void shell.openLibrary(lib.path);
}

/** 从历史移除一条记录（只删记录不动目录；当前库不可移除），浮层原地刷新 */
async function remove(lib: LibraryHistoryItem): Promise<void> {
  const res = await shell.removeLibrary(lib.path);
  current.value = res.current;
  libraries.value = res.libraries;
}

/** 底部「打开文件夹…」：弹系统目录选择框加入新库 */
function pickFolder(): void {
  close();
  void shell.selectLibrary();
}

onBeforeUnmount(() => {
  window.removeEventListener('resize', close);
  window.removeEventListener('scroll', onScroll, true);
  document.removeEventListener('mousedown', onOutside, true);
  document.removeEventListener('keydown', onKeydown, true);
});
</script>

<template>
  <!-- 单根 button：父 scoped 的 .library-name 规则与 in-head/in-body 显隐类均依赖根元素继承；
       Teleport 置于 button 内（不产生实际 DOM），仅为保住单根结构 -->
  <button
    ref="triggerRef"
    class="library-name"
    :class="{ open }"
    :title="store.library?.path + '（点击切换素材库）'"
    @click="open = !open"
    @dblclick.stop
  >
    <Icon name="library" />
    <span class="lib-text">{{ store.library?.name ?? 'hawk' }}</span>
    <Icon name="chevronDown" :size="12" class="lib-chev" />
    <Teleport to="body">
      <div
        v-if="open"
        ref="panelRef"
        class="lib-panel"
        :style="{ left: pos.x + 'px', top: pos.y + 'px', width: pos.width + 'px' }"
      >
      <button
        v-for="lib in libraries"
        :key="lib.path"
        type="button"
        class="lib-item"
        :class="{ selected: lib.path === current, disabled: !lib.exists }"
        :title="lib.path"
        @click="switchTo(lib)"
      >
        <span class="check">{{ lib.path === current ? '✓' : '' }}</span>
        <span class="label">{{ lib.exists ? lib.name : `${lib.name}（已删除）` }}</span>
        <span v-if="lib.path !== current" class="remove" title="从列表中移除" @click.stop="void remove(lib)">
          ×
        </span>
      </button>
      <div class="separator" />
      <button type="button" class="lib-item" @click="pickFolder">
        <span class="check" />
        <span class="label">打开文件夹…</span>
      </button>
      </div>
    </Teleport>
  </button>
</template>

<style scoped>
/* 触发按钮的尺寸/间距由 Sidebar 的 .library-name 规则承担（父 scoped 可作用到子组件根元素）；
   本文件只补下拉态与浮层 */
.library-name.open {
  background: var(--bg-2);
}

.lib-chev {
  transition: transform 0.15s;
}

.library-name.open .lib-chev {
  transform: rotate(180deg);
}

.lib-panel {
  position: fixed;
  /* 设置对话框 170 / 预览浮层 200 / 右键菜单 400；本浮层须盖过侧栏与顶栏 */
  z-index: 300;
  max-height: 280px;
  overflow-y: auto;
  padding: 4px;
  border-radius: 6px;
  background: var(--bg-3);
  border: 1px solid var(--border);
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.5);
  display: flex;
  flex-direction: column;
}

.lib-item {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  padding: 6px 12px;
  border: none;
  border-radius: 4px;
  background: transparent;
  text-align: left;
}

@media (hover: hover) {

.lib-item:hover {
  background: color-mix(in srgb, var(--accent) 35%, transparent);
}
}

.lib-item.disabled {
  opacity: 0.45;
  cursor: default;
}

.lib-item .check {
  width: 12px;
  flex: none;
  color: var(--accent);
}

.lib-item .label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 项右侧移除按钮：默认隐去，悬停该行时浮现，避免整列 × 造成视觉噪音 */
.lib-item .remove {
  flex: none;
  margin-right: -6px;
  padding: 0 4px;
  font-size: 14px;
  line-height: 1;
  color: var(--fg-1);
  visibility: hidden;
}

@media (hover: hover) {

.lib-item:hover .remove {
  visibility: visible;
}
}

@media (hover: hover) {

.lib-item .remove:hover {
  color: var(--danger);
}
}

.separator {
  height: 1px;
  margin: 4px 8px;
  background: var(--border);
}
</style>

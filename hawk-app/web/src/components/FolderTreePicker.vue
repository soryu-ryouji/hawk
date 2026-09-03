<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { onClickOutside, useEventListener } from '@vueuse/core';
import { useTaxonomyStore } from '../stores/taxonomy';
import type { FolderNode } from '../types';

// Eagle 式文件夹树选择弹出层（检查器「文件夹」使用）：点击当前值弹出，
// 点击文件夹行即选中移动（无确认，与 Eagle 一致）；点外部/Esc 关闭。
// 仅编辑版使用——只读端（触屏/viewer）的文件夹是纯文字跳转。
const props = defineProps<{
  /** 当前所在文件夹（"" 为根目录），高亮并默认沿其路径展开 */
  current: string;
  /** 触发按钮（检查器的文件夹当前值按钮）：点击它属于 toggle，不得按「点外部」关闭——
   * 否则 pointerdown 先关、click 后开，再点一次永远关不上 */
  trigger: HTMLElement | null;
  /** 触发按钮的视口定位（Inspector 计算，含下方空间不足时的翻转） */
  anchor: { left: number; width: number; top: number; bottom: number; flip: boolean };
}>();
const emit = defineEmits<{ pick: [path: string]; close: [] }>();

const taxonomy = useTaxonomyStore();
const panelEl = ref<HTMLElement | null>(null);
onClickOutside(
  panelEl,
  () => emit('close'),
  { ignore: [computed(() => props.trigger)] },
);
onMounted(() => panelEl.value?.focus());

// 面板内按键不透传给全局快捷键（Delete/Backspace 不误删素材）；Esc 关闭
useEventListener(panelEl, 'keydown', (e: KeyboardEvent) => {
  e.stopPropagation();
  if (e.key === 'Escape') {
    emit('close');
  }
});

/** 展开态（不持久化）：默认沿当前文件夹路径展开，其余收起 */
const expanded = ref<Set<string>>(new Set());
for (let dir = props.current; dir.includes('/'); dir = dir.slice(0, dir.lastIndexOf('/'))) {
  expanded.value.add(dir);
}

function toggle(path: string) {
  const next = new Set(expanded.value);
  if (next.has(path)) {
    next.delete(path);
  } else {
    next.add(path);
  }
  expanded.value = next;
}

/** 展开过滤后的可见行（平铺渲染，与 taxonomy.flatFolders 同款思路） */
const rows = computed(() => {
  const out: { path: string; name: string; depth: number; hasChildren: boolean }[] = [];
  const walk = (children: FolderNode['children'], depth: number) => {
    for (const child of children) {
      out.push({ path: child.path, name: child.name, depth, hasChildren: child.children.length > 0 });
      if (expanded.value.has(child.path)) {
        walk(child.children, depth + 1);
      }
    }
  };
  if (taxonomy.folders) {
    walk(taxonomy.folders.children, 0);
  }
  return out;
});

/** 选择即移动：点当前所在行视为取消（关闭不移动） */
function pick(path: string) {
  if (path !== props.current) {
    emit('pick', path);
  }
  emit('close');
}
</script>

<template>
  <Teleport to="body">
    <div
      ref="panelEl"
      class="folder-picker"
      tabindex="0"
      :style="
        anchor.flip
          ? { left: `${anchor.left}px`, width: `${anchor.width}px`, bottom: `${anchor.bottom}px` }
          : { left: `${anchor.left}px`, width: `${anchor.width}px`, top: `${anchor.top}px` }
      "
    >
      <div class="picker-title">选择所在文件夹</div>
      <div class="root-row" :class="{ active: current === '' }" @click="pick('')">（根目录）</div>
      <div v-for="row in rows" :key="row.path" class="node" :class="{ active: row.path === current }" :style="{ paddingLeft: 12 + row.depth * 14 + 'px' }" @click="pick(row.path)">
        <span v-if="row.hasChildren" class="arrow" :class="{ expanded: expanded.has(row.path) }" @click.stop="toggle(row.path)">▸</span>
        <span v-else class="arrow-placeholder" />
        <span class="name">{{ row.name }}</span>
      </div>
      <div v-if="rows.length === 0" class="empty">暂无文件夹，可先到侧栏新建</div>
    </div>
  </Teleport>
</template>

<style scoped>
.folder-picker {
  position: fixed;
  z-index: 150;
  max-height: 300px;
  overflow-y: auto;
  padding: 6px 0;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--bg-1);
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.5);
  outline: none;
  font-size: 12px;
}

.picker-title {
  padding: 2px 12px 6px;
  color: var(--fg-1);
  font-size: 11px;
}

.root-row,
.node {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 5px 12px 5px 8px;
  cursor: pointer;
  white-space: nowrap;
}

.root-row:hover,
.node:hover {
  background: var(--bg-2);
}

.root-row.active,
.node.active {
  /* Eagle 式选中高亮:暗灰微亮(--bg-3),不用亮色 accent */
  background: var(--bg-3);
  color: #fff;
}

.arrow {
  display: inline-block;
  width: 14px;
  flex: none;
  text-align: center;
  transition: transform 0.1s;
}

.arrow.expanded {
  transform: rotate(90deg);
}

.arrow-placeholder {
  width: 14px;
  flex: none;
}

.name {
  overflow: hidden;
  text-overflow: ellipsis;
}

.empty {
  padding: 8px 12px;
  color: var(--fg-1);
}
</style>

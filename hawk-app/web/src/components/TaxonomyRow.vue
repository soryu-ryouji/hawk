<script setup lang="ts">
import { useLibraryStore } from '../stores/library';
import { useTaxonomyStore } from '../stores/taxonomy';
import { useContextMenu } from '../composables/useContextMenu';
import Icon from './Icon.vue';

// 侧栏分类/标签共用的行组件：两者结构完全相同（图标/名称/计数/右键菜单/拖入高亮），
// 仅 kind 相关的图标、菜单文案与 API 不同。拖拽 enter/leave/drop 由 Sidebar 容器级委托
// 处理（见 Sidebar.vue），本组件只提供 .tax-row 根与 data-name。
const props = defineProps<{
  kind: 'category' | 'tag';
  name: string;
  /** 条目计数（schema int64 为 number|string，原样透传展示） */
  count: number | string;
  active: boolean;
  dropTarget: boolean;
}>();
const emit = defineEmits<{ rename: [name: string] }>();

const store = useLibraryStore();
const taxonomy = useTaxonomyStore();
const menu = useContextMenu();

function onClick() {
  store.setView(props.kind === 'category' ? { kind: 'category', name: props.name } : { kind: 'tag', name: props.name });
}

/** 右键：重命名/刷新缓存/删除（只读查看 viewer 无写操作菜单，整体不出） */
function onContextMenu(e: MouseEvent) {
  if (store.viewerMode) {
    return;
  }
  const kindLabel = props.kind === 'category' ? '分类' : '标签';
  menu.open(
    [
      { label: '重命名', action: () => emit('rename', props.name) },
      { label: '刷新缓存', title: `修复该${kindLabel}下素材缺失的宽高/缩略图/调色板`, action: () => void store.refreshCache(props.kind, props.name) },
      {
        label: `删除${kindLabel}`,
        danger: true,
        action: () => {
          if (window.confirm(`删除${kindLabel}「${props.name}」？全部素材的该${kindLabel}将被清除。`)) {
            void (props.kind === 'category' ? taxonomy.categoryDelete(props.name) : taxonomy.tagDelete(props.name));
          }
        },
      },
    ],
    e,
  );
}
</script>

<template>
  <div class="tax-row" :class="{ active, 'drop-target': dropTarget }" :data-name="name" @click="onClick" @contextmenu.prevent.stop="onContextMenu">
    <Icon :name="kind === 'category' ? 'category' : 'tag'" :size="13" />
    <span class="tax-name">{{ name }}</span>
    <span class="tax-count">{{ count }}</span>
  </div>
</template>

<style scoped>
/* 左缩进与树节点名称列对齐（12px 内边距 + 14px 箭头占位），三个分区内容同一垂直线 */
.tax-row {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 4px 12px 4px 26px;
  cursor: pointer;
  overflow: hidden;
}

.tax-row:hover {
  background: var(--bg-2);
}

.tax-row.active {
  /* Eagle 式选中高亮:暗灰微亮(--bg-3),不用亮色 accent */
  background: var(--bg-3);
  color: #fff;
}

.tax-row.active .tax-count {
  color: #fff;
  font-weight: 600;
}

/* 素材悬停：整行高亮示意可放置（与 FolderTreeNode 同款） */
.tax-row.drop-target {
  background: color-mix(in srgb, var(--accent) 30%, transparent);
  outline: 1px dashed var(--accent);
  outline-offset: -1px;
}

.tax-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tax-count {
  font-size: 11px;
  color: var(--fg-1);
}
</style>

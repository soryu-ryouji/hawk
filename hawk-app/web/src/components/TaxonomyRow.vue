<script setup lang="ts">
import { computed } from 'vue';
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

/** 全局列表隐藏标记（行尾 eyeOff 图标与右键菜单文案共用） */
const hidden = computed(() => taxonomy.isHidden(props.kind, props.name));

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
      {
        label: hidden.value ? '恢复在全局列表显示' : '不在全局列表显示',
        title: '隐藏后其下素材不再出现在全部素材/根目录/未分类/未标签列表，进入该维度视图仍可见',
        action: () => void taxonomy.setHidden(props.kind, props.name, !hidden.value),
      },
      { label: '刷新缓存', title: `修复该${kindLabel}下素材缺失的宽高/缩略图/调色板，并清除源文件已删除的残留条目`, action: () => void store.refreshCache(props.kind, props.name) },
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
    <Icon v-if="hidden" name="eyeOff" :size="12" class="tax-hidden" />
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

@media (hover: hover) {

.tax-row:hover {
  background: var(--bg-2);
}
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

/* 全局列表隐藏标记（行尾眼睛划线图标） */
.tax-hidden {
  flex: none;
  color: var(--fg-1);
}
</style>

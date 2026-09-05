<script setup lang="ts">
// 隐藏项分区：当前被标记「不在全局列表显示」的文件夹/分类/标签清单，逐条取消隐藏。
// 数据源为 taxonomy store 的全局隐藏集（启动时已加载，SSE 事件实时对齐），无需自行拉取。
// 只读查看（局域网 viewer）只展示不操作（服务端 403 为最终防线）。
// 幽灵条目（指向已不存在维度）由用户在此手动清除——级联跟随之外的兜底出口。
import { computed } from 'vue';
import { useLibraryStore } from '../stores/library';
import { useTaxonomyStore } from '../stores/taxonomy';
import { TRASH_PREFIX, displayPath } from '../viewLogic';
import Icon from './Icon.vue';

const store = useLibraryStore();
const taxonomy = useTaxonomyStore();

interface HiddenRow {
  kind: 'folder' | 'category' | 'tag';
  /** 原始值（取消隐藏时回传服务端） */
  name: string;
  /** 展示名（回收站中的隐藏文件夹剥掉 .hawk/trash/ 前缀并加注） */
  label: string;
  icon: 'folder' | 'category' | 'tag';
}

const rows = computed<HiddenRow[]>(() => {
  const gf = taxonomy.globalFilter;
  return [
    ...gf.folders.map((path) => ({
      kind: 'folder' as const,
      name: path,
      label: path.startsWith(TRASH_PREFIX) ? `${displayPath(path)}（回收站中）` : path,
      icon: 'folder' as const,
    })),
    ...gf.categories.map((name) => ({ kind: 'category' as const, name, label: name, icon: 'category' as const })),
    ...gf.tags.map((name) => ({ kind: 'tag' as const, name, label: name, icon: 'tag' as const })),
  ];
});

const KIND_LABEL: Record<HiddenRow['kind'], string> = { folder: '文件夹', category: '分类', tag: '标签' };
</script>

<template>
  <div class="pane">
    <div class="field column">
      <span class="field-label">全局列表隐藏项</span>
      <p class="hint">
        被隐藏的文件夹/分类/标签，其下素材不再出现在全部素材、根目录、未分类、未标签列表中；进入该维度自身视图仍可见。
      </p>
    </div>

    <div v-if="rows.length === 0" class="hint">暂无隐藏项。在侧栏文件夹/分类/标签上右键可设置「不在全局列表显示」。</div>

    <div v-for="row in rows" :key="`${row.kind}:${row.name}`" class="hiding-row">
      <Icon :name="row.icon" :size="13" />
      <span class="hiding-kind">{{ KIND_LABEL[row.kind] }}</span>
      <span class="hiding-name" :title="row.label">{{ row.label }}</span>
      <button
        v-if="!store.viewerMode"
        class="icon-btn"
        title="恢复在全局列表显示"
        @click="void taxonomy.setHidden(row.kind, row.name, false)"
      >
        <Icon name="close" :size="12" />
      </button>
    </div>
  </div>
</template>

<style scoped>
.hiding-row {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
}

.hiding-kind {
  flex: none;
  font-size: 11px;
  color: var(--fg-1);
}

.hiding-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>

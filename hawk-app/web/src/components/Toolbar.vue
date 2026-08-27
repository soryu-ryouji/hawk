<script setup lang="ts">
import { computed, ref } from 'vue';
import { useLibraryStore } from '../stores/library';
import Icon from './Icon.vue';

const store = useLibraryStore();
const searchText = ref('');

/** Eagle 式：左侧当前位置标题 + 选中计数 */
const locationTitle = computed(() => {
  const view = store.view;
  if (view.kind === 'all') return '全部素材';
  if (view.kind === 'trash') return '回收站';
  if (view.kind === 'folder') return view.path.split('/').pop() ?? view.path;
  if (view.kind === 'category') return view.path.split('/').pop() ?? view.path;
  return view.name;
});

const sortValue = computed({
  get: () => `${store.query.orderBy}:${store.query.order}`,
  set: (value: string) => {
    const [orderBy, order] = value.split(':') as [typeof store.query.orderBy, typeof store.query.order];
    store.setQuery({ orderBy, order });
  },
});

function submitSearch() {
  store.setQuery({ keywords: searchText.value.trim().split(/\s+/).filter(Boolean) });
}

function setStar(e: Event) {
  const value = Number((e.target as HTMLSelectElement).value);
  store.setQuery({ star: value < 0 ? undefined : value });
}
</script>

<template>
  <div class="toolbar">
    <div class="location">
      <span class="title">{{ locationTitle }}</span>
      <span v-if="store.selection.length" class="selected"> · 已选 {{ store.selection.length }} 项</span>
    </div>

    <select :value="store.query.star ?? -1" class="filter" title="评分筛选" @change="setStar">
      <option :value="-1">全部评分</option>
      <option v-for="n in 6" :key="n - 1" :value="n - 1">{{ n - 1 }} 星</option>
    </select>

    <select v-model="sortValue" class="sort" title="排序">
      <option value="modification_time:desc">修改时间 ↓</option>
      <option value="modification_time:asc">修改时间 ↑</option>
      <option value="name:asc">名称 ↑</option>
      <option value="name:desc">名称 ↓</option>
      <option value="size:desc">大小 ↓</option>
      <option value="size:asc">大小 ↑</option>
      <option value="star:desc">评分 ↓</option>
      <option value="star:asc">评分 ↑</option>
    </select>

    <input v-model.number="store.thumbSize" class="thumb-slider" type="range" min="120" max="280" step="8" title="行高（缩略图）" />

    <div class="search-box">
      <Icon name="search" :size="13" />
      <input v-model="searchText" type="search" placeholder="搜索" @keydown.enter="submitSearch" />
    </div>
  </div>
</template>

<style scoped>
.toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 14px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-1);
}

.location {
  min-width: 0;
}

.title {
  font-size: 14px;
  font-weight: 600;
}

.selected {
  color: var(--fg-1);
  font-size: 12px;
}

.filter,
.sort {
  padding: 3px 6px;
  color: var(--fg-1);
}

.thumb-slider {
  flex: 1;
  max-width: 160px;
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
}

.search-box input {
  width: 160px;
  padding: 4px 0;
  border: none;
  background: transparent;
}
</style>

<script setup lang="ts">
import { ref } from 'vue';
import { useLibraryStore } from '../stores/library';
import type { QueryState } from '../types';

const store = useLibraryStore();
const searchText = ref('');

function submitSearch() {
  store.setQuery({ keywords: searchText.value.trim().split(/\s+/).filter(Boolean) });
}

function setStar(e: Event) {
  const value = Number((e.target as HTMLSelectElement).value);
  store.setQuery({ star: value < 0 ? undefined : value });
}

function setOrderBy(e: Event) {
  store.setQuery({ orderBy: (e.target as HTMLSelectElement).value as QueryState['orderBy'] });
}

function toggleOrder() {
  store.setQuery({ order: store.query.order === 'desc' ? 'asc' : 'desc' });
}
</script>

<template>
  <div class="toolbar">
    <input
      v-model="searchText"
      class="search"
      type="search"
      placeholder="搜索名称或备注，回车确认"
      @keydown.enter="submitSearch"
    />

    <select :value="store.query.star ?? -1" title="评分筛选" @change="setStar">
      <option :value="-1">全部评分</option>
      <option v-for="n in 6" :key="n - 1" :value="n - 1">{{ n - 1 }} 星</option>
    </select>

    <select :value="store.query.orderBy" title="排序字段" @change="setOrderBy">
      <option value="modification_time">按修改时间</option>
      <option value="name">按名称</option>
      <option value="size">按大小</option>
      <option value="star">按评分</option>
    </select>

    <button class="order" :title="store.query.order === 'desc' ? '降序' : '升序'" @click="toggleOrder">
      {{ store.query.order === 'desc' ? '↓' : '↑' }}
    </button>

    <input v-model.number="store.thumbSize" class="thumb-slider" type="range" min="96" max="256" step="8" title="缩略图尺寸" />
  </div>
</template>

<style scoped>
.toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-1);
}

.search {
  flex: 1;
  max-width: 360px;
  padding: 4px 10px;
}

.order {
  width: 32px;
  padding: 4px 0;
}

.thumb-slider {
  margin-left: auto;
  width: 120px;
  padding: 0;
  border: none;
  background: transparent;
}
</style>

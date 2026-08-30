<script setup lang="ts">
// 搜索框：TitleBar 与 Inspector 顶部共用。
// 触屏横屏（wide + touch，如 iPad 横屏）时 Inspector 顶实例经 styles.css 显示、顶栏实例隐藏，
// 把顶栏空间留给筛选/排序按钮；其余布局顶栏实例常驻、Inspector 实例隐藏。
import { useLibraryStore } from '../stores/library';
import Icon from './Icon.vue';

const store = useLibraryStore();
</script>

<template>
  <div class="search-box">
    <Icon name="search" :size="13" />
    <input v-model="store.searchText" type="search" placeholder="搜索" @keydown.enter="store.submitSearch()" />
  </div>
</template>

<style scoped>
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

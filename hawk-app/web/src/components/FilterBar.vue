<script setup lang="ts">
// 筛选工具列（Eagle 式）：TitleBar 下方一行的条件筛选 chip。
// 显隐由挂载方（App.vue）控制：点击顶栏漏斗按钮展开，或查询带筛选条件（评分/颜色）时常驻。
// 评分：点击 chip 弹星级菜单；颜色：条件激活时显示色块 chip，可就地清除。
import { useLibraryStore } from '../stores/library';
import { useContextMenu } from '../composables/useContextMenu';
import Icon from './Icon.vue';

const store = useLibraryStore();
const { open: openMenu } = useContextMenu();

/** 评分 chip：弹星级单选菜单（与原下拉同语义：精确匹配，全部 = 清除条件） */
function openRatingMenu(e: MouseEvent) {
  openMenu(
    [
      {
        label: '全部评分',
        checked: store.query.star === undefined,
        action: () => store.setQuery({ star: undefined }),
      },
      ...[0, 1, 2, 3, 4, 5].map((n) => ({
        label: `${n} 星`,
        checked: store.query.star === n,
        action: () => store.setQuery({ star: n }),
      })),
    ],
    e,
  );
}
</script>

<template>
  <div class="filterbar">
    <button
      class="chip"
      :class="{ active: store.query.star !== undefined }"
      title="评分筛选"
      @click="openRatingMenu"
    >
      <Icon name="star" :size="13" />
      <span>{{ store.query.star !== undefined ? `${store.query.star} 星` : '评分' }}</span>
    </button>

    <div v-if="store.query.color" class="chip color-chip" title="颜色筛选">
      <span class="dot" :style="{ background: store.query.color }" />
      <span class="hex">{{ store.query.color }}</span>
      <button class="clear" title="清除颜色筛选" @click="store.setQuery({ color: undefined })">×</button>
    </div>
  </div>
</template>

<style scoped>
.filterbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-1);
}

.chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border-radius: 11px;
  background: var(--bg-3);
  border: none;
  color: var(--fg-1);
  font-size: 12px;
}

.chip:hover {
  color: var(--fg-0);
}

/* 条件激活的 chip 高亮（评分已定值/颜色筛选中） */
.chip.active {
  color: var(--accent);
}

.color-chip .dot {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  border: 1px solid rgba(255, 255, 255, 0.15);
}

.color-chip .clear {
  padding: 0 2px;
  border: none;
  background: transparent;
  color: var(--fg-1);
}

.color-chip .clear:hover {
  color: var(--danger);
  background: transparent;
}
</style>

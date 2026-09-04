<script setup lang="ts">
// 检查器壳：顶部拖拽条（含触屏横屏时的搜索框）+ 按选中数分区分发：
// 单选 → InspectorItem（完整编辑）；多选 → InspectorBatch（批量操作）；无选中 → 分区状态。
import { useLibraryStore } from '../stores/library';
import { formatSize } from '../format';
import { shell } from '../platform';
import SearchBox from './SearchBox.vue';
import InspectorItem from './InspectorItem.vue';
import InspectorBatch from './InspectorBatch.vue';

const store = useLibraryStore();

/** 顶部拖拽条双击切换最大化（与 TitleBar 一致；条内无交互控件，无需排除判断） */
function onHeadDblClick() {
  void shell.toggleMaximizeWindow();
}
</script>

<template>
  <aside class="inspector inspector-scope">
    <!-- 顶部拖拽条：检查器色块通高到窗口上沿；Windows/Linux 自绘窗口控制 fixed 在本条右侧。
         搜索框默认隐藏；触屏横屏（wide+touch）时填充本条（浏览器端本条本是无拖拽需求的空条） -->
    <div class="inspector-head" @dblclick="onHeadDblClick">
      <SearchBox class="inspector-search" @dblclick.stop />
    </div>
    <div class="inspector-body">
      <!-- 单选：完整编辑 -->
      <InspectorItem v-if="store.primarySelected && store.selection.length === 1" />

      <!-- 多选：批量操作 -->
      <InspectorBatch v-else-if="store.selection.length > 1" />

      <!-- 无选中：当前分区状态（参考 Eagle：分区名 + 基本信息） -->
      <div v-else class="section-status">
        <div class="status-title">{{ store.viewTitle }}</div>
        <section>
          <div class="section-title">基本信息</div>
          <dl class="info">
            <dt>文件数</dt>
            <dd>{{ store.total }}</dd>
            <dt>占用空间</dt>
            <dd>{{ formatSize(store.totalSize) }}</dd>
          </dl>
        </section>
      </div>
    </div>
  </aside>
</template>

<style src="./inspector-shared.css"></style>

<style scoped>
.inspector {
  display: flex;
  flex-direction: column;
  background: var(--bg-2);
  border-left: 1px solid var(--border);
  overflow: hidden;
}

.inspector-head {
  flex: none;
  height: 40px;
  display: flex;
  align-items: center;
  padding: 0 8px;
  -webkit-app-region: drag;
}

.inspector-body {
  flex: 1;
  overflow-y: auto;
}

.section-status {
  padding: 12px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.status-title {
  padding: 8px 10px;
  border: 1px solid var(--border);
  border-radius: 5px;
  background: var(--bg-3);
  font-weight: 600;
}
</style>

<script setup lang="ts">
// 检查器无选中态：当前视图概览（参考 Eagle：分区名 + 基本信息，数据取自 store 的 viewTitle/total/totalSize）。
// 与单选 InspectorItem / 多选 InspectorBatch 同级的第三种形态，由壳按选中数分发。
import { useLibraryStore } from '../../store';
import { formatSize } from '@/shared/lib/format';

const store = useLibraryStore();
</script>

<template>
  <div class="section-status">
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
</template>

<style src="./inspector-shared.css"></style>

<style scoped>
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

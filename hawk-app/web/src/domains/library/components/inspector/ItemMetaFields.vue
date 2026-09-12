<script setup lang="ts">
// 检查器「基本信息」区（哑组件）：只读/编辑共用一份结构。
// 评分行默认纯文本星（只读态展示）；编辑版经默认插槽注入 StarRating 控件。
import { formatSize, formatTime } from '@/shared/lib/format';
import type { Item } from '@/shared/types';

defineProps<{ item: Item }>();
</script>

<template>
  <dl class="info">
    <dt>评分</dt>
    <dd>
      <slot>{{ Number(item.star) > 0 ? '★'.repeat(Number(item.star)) : '—' }}</slot>
    </dd>
    <dt>尺寸</dt>
    <dd>{{ item.width }} × {{ item.height }}</dd>
    <dt>文件大小</dt>
    <dd>{{ formatSize(Number(item.size)) }}</dd>
    <dt>格式</dt>
    <dd>{{ item.ext.toUpperCase() }}</dd>
    <dt>修改时间</dt>
    <dd>{{ formatTime(Number(item.modification_time)) }}</dd>
    <dt>ID</dt>
    <dd :title="item.id">{{ item.id.slice(0, 12) }}…</dd>
  </dl>
</template>

<style src="./inspector-shared.css"></style>

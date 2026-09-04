<script setup lang="ts">
// 检查器多选区：批量操作面板（参考 Eagle 多选面板）。只读查看下隐藏全部写操作，
// 仅保留堆叠预览与基本信息。
import { computed, ref } from 'vue';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
import { useTaxonomyStore } from '../stores/taxonomy';
import { formatSize } from '../format';
import { itemKey } from '../viewLogic';
import StarRating from './StarRating.vue';
import CategoryPickerDialog from './CategoryPickerDialog.vue';
import FolderPickerDialog from './FolderPickerDialog.vue';

const store = useLibraryStore();
const taxonomy = useTaxonomyStore();
const showCategoryPicker = ref(false);
const showFolderPicker = ref(false);
const batchTag = ref('');

const totalSelectedSize = computed(() =>
  store.selectedItems.reduce((sum, selected) => sum + Number(selected.size), 0),
);

function applyStarToAll(value: number) {
  void store.setStarForSelected(value);
}

function applyBatchTag() {
  const tag = batchTag.value.trim();
  if (tag) {
    store.addTagToSelected(tag);
  }
  batchTag.value = '';
}

function batchAddCategory(name: string) {
  showCategoryPicker.value = false;
  store.addCategoryToSelected(name);
}

function batchMoveFolder(path: string) {
  showFolderPicker.value = false;
  store.moveSelectedToFolder(path);
}
</script>

<template>
  <div class="multi">
    <div class="stack">
      <img
        v-for="(selected, i) in store.selectedItems.slice(0, 3)"
        :key="itemKey(selected.id, selected.path)"
        :src="api.thumbnailUrl(selected.id)"
        :style="{ zIndex: 3 - i, transform: `translateX(${i * 18}px) rotate(${(i - 1) * 5}deg)` }"
        alt=""
      />
    </div>

    <p class="multi-title">已选 <b>{{ store.selection.length }}</b> 个文件</p>

    <template v-if="!store.viewerMode">
      <section>
        <div class="section-title">标签</div>
        <input
          v-model="batchTag"
          list="batch-tag-suggestions"
          placeholder="＋ 添加标签（应用到全部选中）"
          @keydown.enter="applyBatchTag"
        />
        <datalist id="batch-tag-suggestions">
          <option v-for="t in taxonomy.tagList" :key="t.name" :value="t.name" />
        </datalist>
      </section>

      <section>
        <div class="section-title">分类</div>
        <button class="batch-btn" @click="showCategoryPicker = true">＋ 添加到分类</button>
      </section>

      <section>
        <div class="section-title">文件夹</div>
        <button class="batch-btn" @click="showFolderPicker = true">＋ 移动到文件夹</button>
      </section>

      <section>
        <div class="section-title">基本信息</div>
        <dl class="info">
          <dt>评分</dt>
          <dd><StarRating :model-value="0" @update:model-value="applyStarToAll" /></dd>
          <dt>文件大小</dt>
          <dd>{{ formatSize(totalSelectedSize) }}</dd>
        </dl>
      </section>

      <button v-if="!store.isTrash" class="danger" @click="store.trashSelected()">移入回收站</button>
      <button v-else @click="store.restoreSelected()">恢复</button>
    </template>

    <CategoryPickerDialog v-if="showCategoryPicker" title="添加到分类" @confirm="batchAddCategory" @cancel="showCategoryPicker = false" />
    <FolderPickerDialog v-if="showFolderPicker" title="移动到文件夹" @confirm="batchMoveFolder" @cancel="showFolderPicker = false" />
  </div>
</template>

<style src="./inspector-shared.css"></style>

<style scoped>
.multi {
  padding: 16px 12px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}

.stack {
  position: relative;
  height: 140px;
  display: flex;
  justify-content: center;
  align-items: center;
}

.stack img {
  position: absolute;
  max-width: 120px;
  max-height: 130px;
  border-radius: 4px;
  border: 2px solid var(--bg-2);
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.5);
  object-fit: cover;
}

.multi-title {
  text-align: center;
  font-size: 14px;
}

.multi-title b {
  color: var(--accent);
}

.batch-btn {
  width: 100%;
  padding: 7px 10px;
  text-align: center;
}

.multi section input {
  width: 100%;
  padding: 6px 8px;
}
</style>

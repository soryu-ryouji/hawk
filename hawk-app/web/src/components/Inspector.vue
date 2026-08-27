<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
import TagEditor from './TagEditor.vue';
import StarRating from './StarRating.vue';
import CategoryPickerDialog from './CategoryPickerDialog.vue';

const store = useLibraryStore();
const showCategoryPicker = ref(false);

// 编辑字段为本地副本，切换选中项时重置；失焦/回车提交
const name = ref('');
const annotation = ref('');
const url = ref('');
const tags = ref<string[]>([]);
const star = ref(0);

const item = computed(() => store.primarySelected);
const previewUrl = computed(() => (item.value ? api.thumbnailUrl(item.value.id, 1024) : ''));

watch(
  () => item.value?.id,
  () => {
    name.value = item.value?.name ?? '';
    annotation.value = item.value?.annotation ?? '';
    url.value = item.value?.url ?? '';
    tags.value = [...(item.value?.tags ?? [])];
    star.value = Number(item.value?.star ?? 0);
  },
  { immediate: true },
);

// SSE/其他来源更新了当前 item 时同步本地副本（编辑中的字段以服务器值为准）
watch(item, (fresh) => {
  if (fresh && document.activeElement?.tagName !== 'INPUT' && document.activeElement?.tagName !== 'TEXTAREA') {
    name.value = fresh.name;
    annotation.value = fresh.annotation ?? '';
    url.value = fresh.url ?? '';
    tags.value = [...(fresh.tags ?? [])];
    star.value = Number(fresh.star);
  }
});

function submitName() {
  const value = name.value.trim();
  if (item.value && value && value !== item.value.name) {
    void store.updateItem(item.value.id, { name: value });
  }
}

function submitAnnotation() {
  if (item.value && annotation.value !== (item.value.annotation ?? '')) {
    void store.updateItem(item.value.id, { annotation: annotation.value });
  }
}

function submitUrl() {
  if (item.value && url.value !== (item.value.url ?? '')) {
    void store.updateItem(item.value.id, { url: url.value });
  }
}

watch(tags, (value) => {
  if (item.value && JSON.stringify(value) !== JSON.stringify(item.value.tags)) {
    void store.updateItem(item.value.id, { tags: value });
  }
});

watch(star, (value) => {
  if (item.value && value !== item.value.star) {
    void store.updateItem(item.value.id, { star: value });
  }
});

function formatSize(bytes: number): string {
  if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(1) + ' MB';
  if (bytes >= 1 << 10) return (bytes / (1 << 10)).toFixed(1) + ' KB';
  return bytes + ' B';
}

function formatTime(ms: number): string {
  return new Date(Number(ms)).toLocaleString();
}

function showInFinder(path: string) {
  void window.hawkShell?.showInFinder(path);
}

function removeCategory(category: string) {
  if (item.value) {
    void store.updateItem(item.value.id, {
      categories: (item.value.categories ?? []).filter((c) => c !== category),
    });
  }
}

function addCategory(path: string) {
  showCategoryPicker.value = false;
  if (item.value && !(item.value.categories ?? []).includes(path)) {
    void store.updateItem(item.value.id, { categories: [...(item.value.categories ?? []), path] });
  }
}

/** 库内相对路径的父文件夹（"" 为根目录） */
function folderOf(relPath: string): string {
  const idx = relPath.lastIndexOf('/');
  return idx < 0 ? '' : relPath.slice(0, idx);
}

function submitFolder(path: string) {
  if (item.value && folderOf(item.value.paths[0]) !== path) {
    void store.updateItem(item.value.id, { folder_path: path });
  }
}

function applyStarToAll(value: number) {
  for (const selected of store.selectedItems) {
    void store.updateItem(selected.id, { star: value });
  }
}
</script>

<template>
  <aside class="inspector">
    <!-- 单选：完整编辑 -->
    <template v-if="item && store.selection.length === 1">
      <div class="preview">
        <img :src="previewUrl" :alt="item.name" draggable="false" />
      </div>

      <div class="fields">
        <input v-model="name" class="name-input" title="重命名文件" @keydown.enter="submitName" @blur="submitName" />

        <div class="row">
          <StarRating v-model="star" />
        </div>

        <TagEditor v-model="tags" />

        <div class="row cats">
          <span class="row-label">分类</span>
          <div class="cat-chips">
            <span v-for="category in item.categories ?? []" :key="category" class="chip">
              {{ category }}
              <button class="remove" title="移出该分类" @click="removeCategory(category)">×</button>
            </span>
            <button class="add-cat" title="添加到分类" @click="showCategoryPicker = true">＋</button>
          </div>
        </div>

        <textarea
          v-model="annotation"
          class="annotation"
          placeholder="备注"
          rows="3"
          @blur="submitAnnotation"
        />

        <input v-model="url" placeholder="来源网址" @keydown.enter="submitUrl" @blur="submitUrl" />

        <div class="row">
          <span class="row-label">文件夹</span>
          <select
            class="folder-select"
            :value="folderOf(item.paths[0])"
            :disabled="store.isTrash"
            @change="submitFolder(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="folder in store.flatFolders" :key="folder.path" :value="folder.path">
              {{ folder.label }}
            </option>
          </select>
        </div>

        <dl class="info">
          <dt>尺寸</dt>
          <dd>{{ item.width }} × {{ item.height }}</dd>
          <dt>大小</dt>
          <dd>{{ formatSize(Number(item.size)) }}</dd>
          <dt>格式</dt>
          <dd>{{ item.ext }}</dd>
          <dt>修改时间</dt>
          <dd>{{ formatTime(Number(item.modification_time)) }}</dd>
          <dt>ID</dt>
          <dd :title="item.id">{{ item.id.slice(0, 12) }}…</dd>
        </dl>

        <div class="paths">
          <div class="paths-title">文件位置</div>
          <div v-for="path in item.paths" :key="path" class="path-row">
            <span class="path" :title="path">{{ path }}</span>
            <button class="finder" title="在 Finder 中显示" @click="showInFinder(path)">◎</button>
          </div>
        </div>
      </div>
    </template>

    <!-- 多选：批量操作 -->
    <div v-else-if="store.selection.length > 1" class="multi">
      <p>已选 {{ store.selection.length }} 项</p>
      <StarRating :model-value="0" @update:model-value="applyStarToAll" />
      <button v-if="!store.isTrash" class="danger" @click="store.trashSelected()">移入回收站</button>
      <button v-else @click="store.restoreSelected()">恢复</button>
    </div>

    <div v-else class="hint">选择素材查看详情</div>

    <CategoryPickerDialog
      v-if="showCategoryPicker"
      title="添加到分类"
      @confirm="addCategory"
      @cancel="showCategoryPicker = false"
    />
  </aside>
</template>

<style scoped>
.inspector {
  background: var(--bg-2);
  border-left: 1px solid var(--border);
  overflow-y: auto;
}

.preview {
  padding: 12px;
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 160px;
  max-height: 280px;
}

.preview img {
  max-width: 100%;
  max-height: 256px;
  object-fit: contain;
}

.fields {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 0 12px 12px;
}

.name-input {
  font-weight: 600;
  padding: 4px 8px;
}

.annotation {
  padding: 6px 8px;
  resize: vertical;
}

.row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.row-label {
  flex: none;
  font-size: 12px;
  color: var(--fg-1);
}

.cats {
  align-items: flex-start;
}

.cat-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}

.cat-chips .chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 4px 2px 8px;
  border-radius: 10px;
  background: var(--bg-3);
  font-size: 12px;
}

.cat-chips .remove {
  padding: 0 4px;
  border: none;
  background: transparent;
  color: var(--fg-1);
}

.cat-chips .remove:hover {
  color: var(--danger);
  background: transparent;
}

.add-cat {
  padding: 0 8px;
  border-radius: 10px;
  font-size: 12px;
  line-height: 1.6;
}

.folder-select {
  flex: 1;
  min-width: 0;
  padding: 4px 6px;
}

.fields > input:not(.name-input) {
  padding: 4px 8px;
}

.info {
  display: grid;
  grid-template-columns: 64px 1fr;
  gap: 4px 8px;
  font-size: 12px;
  color: var(--fg-1);
}

.info dd {
  color: var(--fg-0);
}

.paths-title {
  font-size: 12px;
  color: var(--fg-1);
  margin-bottom: 4px;
}

.path-row {
  display: flex;
  align-items: center;
  gap: 6px;
}

.path {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  color: var(--fg-1);
}

.finder {
  padding: 0 6px;
  border: none;
  background: transparent;
  color: var(--fg-1);
}

.finder:hover {
  color: var(--accent);
  background: transparent;
}

.multi {
  padding: 16px 12px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  align-items: flex-start;
}

.hint {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--fg-1);
}
</style>

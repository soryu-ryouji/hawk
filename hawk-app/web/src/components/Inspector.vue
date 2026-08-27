<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
import { showInFileManagerLabel } from '../platform';
import TagEditor from './TagEditor.vue';
import StarRating from './StarRating.vue';
import CategoryPickerDialog from './CategoryPickerDialog.vue';
import FolderPickerDialog from './FolderPickerDialog.vue';

const store = useLibraryStore();
const showCategoryPicker = ref(false);
const showFolderPicker = ref(false);

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
  if (item.value && JSON.stringify(value) !== JSON.stringify(item.value.tags ?? [])) {
    void store.updateItem(item.value.id, { tags: value });
  }
});

watch(star, (value) => {
  if (item.value && value !== Number(item.value.star)) {
    void store.updateItem(item.value.id, { star: value });
  }
});

// ---- 分类 ----

function removeCategory(category: string) {
  if (item.value) {
    void store.updateItem(item.value.id, {
      categories: (item.value.categories ?? []).filter((c) => c !== category),
    });
  }
}

// ---- 文件夹 ----

function moveToRoot() {
  if (item.value) {
    void store.updateItem(item.value.id, { folder_path: '' });
  }
}

// ---- 其他 ----

function showInFinder(path: string) {
  void window.hawkShell?.showInFinder(path);
}

function formatSize(bytes: number): string {
  if (bytes >= 1 << 20) return (bytes / (1 << 20)).toFixed(2) + ' MB';
  if (bytes >= 1 << 10) return (bytes / (1 << 10)).toFixed(1) + ' KB';
  return bytes + ' B';
}

function formatTime(ms: number): string {
  return new Date(Number(ms)).toLocaleString();
}

function applyStarToAll(value: number) {
  for (const selected of store.selectedItems) {
    void store.updateItem(selected.id, { star: value });
  }
}

// ---- 多选批量 ----

const batchTag = ref('');

const totalSelectedSize = computed(() =>
  store.selectedItems.reduce((sum, selected) => sum + Number(selected.size), 0),
);

function applyBatchTag() {
  const tag = batchTag.value.trim();
  if (tag) {
    store.addTagToSelected(tag);
  }
  batchTag.value = '';
}

function batchAddCategory(path: string) {
  showCategoryPicker.value = false;
  store.addCategoryToSelected(path);
}

function batchMoveFolder(path: string) {
  showFolderPicker.value = false;
  store.moveSelectedToFolder(path);
}
</script>

<template>
  <aside class="inspector">
    <!-- 单选：完整编辑（布局参考 Eagle） -->
    <template v-if="item && store.selection.length === 1">
      <div class="preview">
        <span class="ext-badge">{{ item.ext.toUpperCase() }}</span>
        <img :src="previewUrl" :alt="item.name" draggable="false" />
      </div>

      <div class="fields">
        <input v-model="name" class="name-input" title="重命名文件" @keydown.enter="submitName" @blur="submitName" />

        <input v-model="annotation" placeholder="添加注释" @keydown.enter="submitAnnotation" @blur="submitAnnotation" />

        <input v-model="url" placeholder="来源网址" @keydown.enter="submitUrl" @blur="submitUrl" />

        <section>
          <div class="section-title">标签</div>
          <TagEditor v-model="tags" />
        </section>

        <section>
          <div class="section-title">分类</div>
          <div class="chips">
            <span v-for="category in item.categories ?? []" :key="category" class="chip">
              {{ category }}
              <button class="remove" title="移出该分类" @click="removeCategory(category)">×</button>
            </span>
            <button class="add" title="添加到分类" @click="showCategoryPicker = true">＋</button>
          </div>
        </section>

        <section>
          <div class="section-title">文件夹</div>
          <div class="chips">
            <span v-for="folder in item.folders ?? []" :key="folder" class="chip">
              {{ folder || '（根目录）' }}
              <button class="remove" title="移到根目录" @click="moveToRoot">×</button>
            </span>
            <span v-if="(item.folders ?? []).length === 0" class="chip">（根目录）</span>
            <button class="add" title="移动到文件夹" @click="showFolderPicker = true">＋</button>
          </div>
        </section>

        <section>
          <div class="section-title">基本信息</div>
          <dl class="info">
            <dt>评分</dt>
            <dd><StarRating v-model="star" /></dd>
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
        </section>

        <section>
          <div class="section-title">文件位置</div>
          <div v-for="path in item.paths" :key="path" class="path-row">
            <span class="path" :title="path">{{ path }}</span>
            <button class="finder" :title="showInFileManagerLabel" @click="showInFinder(path)">◎</button>
          </div>
        </section>
      </div>
    </template>

    <!-- 多选：批量操作（参考 Eagle 多选面板） -->
    <div v-else-if="store.selection.length > 1" class="multi">
      <div class="stack">
        <img
          v-for="(selected, i) in store.selectedItems.slice(0, 3)"
          :key="selected.id"
          :src="api.thumbnailUrl(selected.id)"
          :style="{ zIndex: 3 - i, transform: `translateX(${i * 18}px) rotate(${(i - 1) * 5}deg)` }"
          alt=""
        />
      </div>

      <p class="multi-title">已选 <b>{{ store.selection.length }}</b> 个文件</p>

      <section>
        <div class="section-title">标签</div>
        <input
          v-model="batchTag"
          list="batch-tag-suggestions"
          placeholder="＋ 添加标签（应用到全部选中）"
          @keydown.enter="applyBatchTag"
        />
        <datalist id="batch-tag-suggestions">
          <option v-for="t in store.tagList" :key="t.name" :value="t.name" />
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
    </div>

    <div v-else class="hint">选择素材查看详情</div>

    <CategoryPickerDialog v-if="showCategoryPicker" title="添加到分类" @confirm="batchAddCategory" @cancel="showCategoryPicker = false" />
    <FolderPickerDialog v-if="showFolderPicker" title="移动到文件夹" @confirm="batchMoveFolder" @cancel="showFolderPicker = false" />
  </aside>
</template>

<style scoped>
.inspector {
  background: var(--bg-2);
  border-left: 1px solid var(--border);
  overflow-y: auto;
}

.preview {
  position: relative;
  padding: 12px;
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 160px;
  max-height: 280px;
  background: #171717;
}

.preview img {
  max-width: 100%;
  max-height: 256px;
  object-fit: contain;
}

.ext-badge {
  position: absolute;
  top: 18px;
  left: 18px;
  padding: 1px 6px;
  border-radius: 3px;
  background: rgba(0, 0, 0, 0.55);
  color: #fff;
  font-size: 10px;
  letter-spacing: 0.5px;
}

.fields {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
}

.fields > input {
  padding: 5px 8px;
}

.name-input {
  font-weight: 600;
}

section {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.section-title {
  font-size: 12px;
  color: var(--fg-1);
}

.chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}

.chip {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 4px 2px 8px;
  border-radius: 10px;
  background: var(--bg-3);
  font-size: 12px;
}

.chip .remove {
  padding: 0 4px;
  border: none;
  background: transparent;
  color: var(--fg-1);
}

.chip .remove:hover {
  color: var(--danger);
  background: transparent;
}

.add {
  padding: 0 8px;
  border-radius: 10px;
  font-size: 12px;
  line-height: 1.6;
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

.hint {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--fg-1);
}
</style>

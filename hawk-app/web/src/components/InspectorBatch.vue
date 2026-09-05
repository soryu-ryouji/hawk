<script setup lang="ts">
// 检查器多选区：与单选面板（InspectorItem）同构的分区（标签/分类/文件夹/基本信息），
// Unity Inspector 语义——值为全部选中项的交集，混值显示「多个值」，操作按批量应用
// （加/摘标签分类、统一移动、统一评分；摘除走 remove_tags/remove_categories 并集移除）。
// 交集数据源：标签/分类经服务端 item/aggregate（选择集可达数万项，详情缓存只覆盖视口），
// 文件夹/评分/大小按选择集与骨架纯前端计算。
import { computed, nextTick, ref } from 'vue';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
import { useTaxonomyStore } from '../stores/taxonomy';
import { formatSize } from '../format';
import { itemKey, selectionTotalSize, commonFoldersOf, commonStarOf } from '../viewLogic';
import StarRating from './StarRating.vue';
import CategoryPickerDialog from './CategoryPickerDialog.vue';
import FolderTreePicker from './FolderTreePicker.vue';

const store = useLibraryStore();
const taxonomy = useTaxonomyStore();

// ---- 共有特性（交集）----
const commonTags = computed(() => store.selectionAggregate?.tags ?? []);
const commonCategories = computed(() => store.selectionAggregate?.categories ?? []);
/** 共有文件夹：全部选中项同目录 → [目录]；跨目录 → []（混值） */
const commonFolders = computed(() => commonFoldersOf(store.selection));
const commonStar = computed(() => commonStarOf(store.selection, store.skeleton));
const totalSelectedSize = computed(() => selectionTotalSize(store.selection, store.skeletonSizeMap));

/** 跳转对应维度视图（已在该视图时不重查） */
function goView(v: { kind: 'category' | 'tag'; name: string } | { kind: 'folder'; path: string }) {
  if (JSON.stringify(store.view) !== JSON.stringify(v)) {
    store.setView(v);
  }
}

// ---- 标签（＋ 展开输入，Enter/失焦提交，Esc 取消；与 TagEditor 同交互） ----
const tagEditing = ref(false);
const tagInput = ref('');
const tagInputEl = ref<HTMLInputElement>();

async function startTagEdit() {
  tagEditing.value = true;
  await nextTick();
  tagInputEl.value?.focus();
}

function commitTag() {
  const tag = tagInput.value.trim();
  if (tag) {
    store.addTagToSelected(tag);
  }
  tagInput.value = '';
  tagEditing.value = false;
}

// ---- 分类（＋ 弹选择框） ----
const showCategoryPicker = ref(false);

function batchAddCategory(name: string) {
  showCategoryPicker.value = false;
  store.addCategoryToSelected(name);
}

// ---- 文件夹（点击当前值弹树选择，选择即统一移动；混值时显示「多个值」仍可统一） ----
const folderValueEl = ref<HTMLButtonElement | null>(null);
/** 树选择弹出层锚点（null = 关闭）；flip = 触发按钮下方空间不足，向上弹出 */
const pickerAnchor = ref<{ left: number; width: number; top: number; bottom: number; flip: boolean } | null>(null);

function toggleFolderPicker() {
  if (pickerAnchor.value) {
    pickerAnchor.value = null;
    return;
  }
  const rect = folderValueEl.value?.getBoundingClientRect();
  if (!rect) {
    return;
  }
  pickerAnchor.value = {
    left: rect.left,
    width: rect.width,
    top: rect.bottom + 4,
    bottom: window.innerHeight - rect.top + 4,
    flip: rect.bottom + 308 > window.innerHeight,
  };
}

function batchMoveFolder(path: string) {
  pickerAnchor.value = null;
  store.moveSelectedToFolder(path);
}

// ---- 基本信息 ----
function applyStarToAll(value: number) {
  void store.setStarForSelected(value);
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
        <div class="chips">
          <span v-for="tag in commonTags" :key="tag" class="chip">
            <button class="jump" :title="`查看标签「${tag}」`" @click="goView({ kind: 'tag', name: tag })">{{ tag }}</button>
            <button class="remove" :title="`从全部选中项移除标签「${tag}」`" @click="store.removeTagFromSelected(tag)">×</button>
          </span>
          <input
            v-if="tagEditing"
            ref="tagInputEl"
            v-model="tagInput"
            list="batch-tag-suggestions"
            class="tag-input"
            placeholder="标签名（应用到全部选中）"
            @keydown.enter.prevent="commitTag"
            @keydown.esc.prevent="tagInput = ''; tagEditing = false"
            @blur="commitTag"
          />
          <button v-else class="add" title="添加标签（应用到全部选中）" @click="startTagEdit">＋</button>
        </div>
        <datalist id="batch-tag-suggestions">
          <option v-for="t in taxonomy.tagList" :key="t.name" :value="t.name" />
        </datalist>
      </section>

      <section>
        <div class="section-title">分类</div>
        <div class="chips">
          <span v-for="category in commonCategories" :key="category" class="chip">
            <button class="jump" :title="`查看分类「${category}」`" @click="goView({ kind: 'category', name: category })">{{ category }}</button>
            <button class="remove" :title="`从全部选中项移除分类「${category}」`" @click="store.removeCategoryFromSelected(category)">×</button>
          </span>
          <button class="add" title="添加到分类（应用到全部选中）" @click="showCategoryPicker = true">＋</button>
        </div>
      </section>

      <section>
        <div class="section-title">文件夹</div>
        <div class="folder-row">
          <!-- 混值（跨目录）显示「多个值」，点击仍可统一移动 -->
          <button ref="folderValueEl" class="folder-value" title="选择所在文件夹（点击统一移动全部选中项）" @click="toggleFolderPicker">
            {{ commonFolders.length ? commonFolders[0] || '（根目录）' : '多个值' }}
          </button>
        </div>
        <FolderTreePicker
          v-if="pickerAnchor"
          :current="commonFolders[0] ?? ''"
          :trigger="folderValueEl"
          :anchor="pickerAnchor"
          @pick="batchMoveFolder"
          @close="pickerAnchor = null"
        />
      </section>

      <section>
        <div class="section-title">基本信息</div>
        <dl class="info">
          <dt>评分</dt>
          <dd :title="commonStar === null ? '多个值：选中项评分不一，点选即统一' : '应用到全部选中'">
            <StarRating :model-value="commonStar ?? 0" @update:model-value="applyStarToAll" />
          </dd>
          <dt>文件大小</dt>
          <dd>{{ formatSize(totalSelectedSize) }}（合计）</dd>
        </dl>
      </section>

      <button v-if="!store.isTrash" class="danger" @click="store.trashSelected()">移入回收站</button>
      <button v-else @click="store.restoreSelected()">恢复</button>
    </template>

    <!-- 只读查看（局域网 viewer）：同结构纯展示 -->
    <template v-else>
      <section>
        <div class="section-title">标签</div>
        <div v-if="commonTags.length" class="chips">
          <span v-for="tag in commonTags" :key="tag" class="chip">
            <button class="jump" @click="goView({ kind: 'tag', name: tag })">{{ tag }}</button>
          </span>
        </div>
        <span v-else class="ro-empty">—</span>
      </section>
      <section>
        <div class="section-title">分类</div>
        <div v-if="commonCategories.length" class="chips">
          <span v-for="category in commonCategories" :key="category" class="chip">
            <button class="jump" @click="goView({ kind: 'category', name: category })">{{ category }}</button>
          </span>
        </div>
        <span v-else class="ro-empty">—</span>
      </section>
      <section>
        <div class="section-title">文件夹</div>
        <button v-if="commonFolders.length" class="jump ro-text" @click="goView({ kind: 'folder', path: commonFolders[0] })">
          {{ commonFolders[0] || '（根目录）' }}
        </button>
        <span v-else class="ro-text">多个值</span>
      </section>
      <section>
        <div class="section-title">基本信息</div>
        <dl class="info">
          <dt>评分</dt>
          <dd>{{ commonStar !== null && commonStar > 0 ? '★'.repeat(commonStar) : '—' }}</dd>
          <dt>文件大小</dt>
          <dd>{{ formatSize(totalSelectedSize) }}（合计）</dd>
        </dl>
      </section>
    </template>

    <CategoryPickerDialog v-if="showCategoryPicker" title="添加到分类" @confirm="batchAddCategory" @cancel="showCategoryPicker = false" />
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

.tag-input {
  width: 140px;
  padding: 1px 6px;
  font-size: 12px;
}

.add {
  padding: 0 8px;
  border-radius: 10px;
  font-size: 12px;
  line-height: 1.6;
}

/* 与单选面板同款：文件夹当前值撑满整行（FolderTreePicker 的锚宽取自它） */
.folder-row {
  display: flex;
}

.folder-value {
  flex: 1;
  min-width: 0;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: 5px;
  background: var(--bg-3);
  color: var(--fg-0);
  font-size: 12px;
  text-align: left;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

@media (hover: hover) {

.folder-value:hover {
  border-color: var(--accent);
}
}
</style>

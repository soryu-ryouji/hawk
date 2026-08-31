<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
import { useLayout } from '../composables/useLayout';
import { showInFileManagerLabel } from '../platform';
import TagEditor from './TagEditor.vue';
import StarRating from './StarRating.vue';
import SearchBox from './SearchBox.vue';
import CategoryPickerDialog from './CategoryPickerDialog.vue';
import FolderPickerDialog from './FolderPickerDialog.vue';

const store = useLibraryStore();
const { touch } = useLayout();
// 触屏设备（iPad/手机网页，实际生效于 wide 布局的 iPad 横屏；narrow 下检查器隐藏）检查器只读：
// 信息与桌面版一致但全部静态展示——触屏上编辑控件易误触，且网页端浏览场景不希望改动素材库
const readOnly = computed(() => touch.value);
const showCategoryPicker = ref(false);
const showFolderPicker = ref(false);

/** 顶部拖拽条双击切换最大化（与 TitleBar 一致；条内无交互控件，无需排除判断） */
function onHeadDblClick() {
  void window.hawkShell?.toggleMaximizeWindow();
}

// 编辑字段为本地副本，切换选中项时重置；失焦/回车提交
const name = ref('');
const annotation = ref('');
const url = ref('');
const tags = ref<string[]>([]);
const star = ref(0);

// 名称与注释为自动增高的 textarea，内容长时换行显示完整
const nameArea = ref<HTMLTextAreaElement>();
const annotationArea = ref<HTMLTextAreaElement>();

function fit(el?: HTMLTextAreaElement) {
  if (el) {
    el.style.height = 'auto';
    el.style.height = el.scrollHeight + 'px';
  }
}

watch([name, annotation], async () => {
  await nextTick();
  fit(nameArea.value);
  fit(annotationArea.value);
});

const item = computed(() => store.primarySelected);
const previewUrl = computed(() => (item.value ? api.thumbnailUrl(item.value.id) : ''));

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
  // 文件名不允许换行：粘贴的多行文本以空格连接
  const value = name.value.replace(/[\r\n]+/g, ' ').trim();
  name.value = value;
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

/** 单值排他语义，用下拉框而非 chip：当前值取主位置（folders[0]），切换即移动文件 */
const folderOptions = computed(() => {
  const current = item.value?.folders?.[0] ?? '';
  const options = [...store.flatFolders];
  if (current !== '' && !options.some((f) => f.path === current)) {
    options.push({ path: current, label: current }); // 树里已不存在的兜底（外部删除等）
  }
  return options;
});

function moveToFolder(path: string) {
  if (item.value && path !== (item.value.folders?.[0] ?? '')) {
    void store.updateItem(item.value.id, { folder_path: path });
  }
}

// ---- 其他 ----

function showInFinder(path: string) {
  void window.hawkShell?.showInFinder(path);
}

/** 点击色块：在当前视图范围内按颜色检索；再次点击当前检索色则清除 */
function searchColor(color: string) {
  store.setQuery({ color: store.query.color === color ? undefined : color });
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
  void store.setStarForSelected(value);
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
  <aside class="inspector">
    <!-- 顶部拖拽条：检查器色块通高到窗口上沿；Windows/Linux 自绘窗口控制 fixed 在本条右侧 -->
    <!-- 顶部拖拽条：检查器色块通高到窗口上沿；Windows/Linux 自绘窗口控制 fixed 在本条右侧。
         搜索框默认隐藏；触屏横屏（wide+touch）时填充本条（浏览器端本条本是无拖拽需求的空条） -->
    <div class="inspector-head" @dblclick="onHeadDblClick">
      <SearchBox class="inspector-search" @dblclick.stop />
    </div>
    <div class="inspector-body">
    <!-- 单选：完整编辑（布局参考 Eagle） -->
    <template v-if="item && store.selection.length === 1">
      <div class="preview">
        <span class="ext-badge">{{ item.ext.toUpperCase() }}</span>
        <img :src="previewUrl" :alt="item.name" draggable="false" />
      </div>

      <div v-if="item.palette?.length" class="palette">
        <button
          v-for="p in item.palette"
          :key="p.color"
          class="swatch"
          :class="{ active: store.query.color === p.color }"
          :style="{ background: p.color }"
          :title="`${p.color} (${p.percentage}%)`"
          @click="searchColor(p.color)"
        />
      </div>

      <!-- 触屏只读：与桌面版同样的信息结构，全部静态展示，不可修改 -->
      <div v-if="readOnly" class="fields ro-fields">
        <div class="ro-name">{{ item.name }}</div>
        <div v-if="item.annotation" class="ro-annotation">{{ item.annotation }}</div>
        <a v-if="item.url" class="ro-url" :href="item.url" target="_blank" rel="noreferrer">{{ item.url }}</a>

        <section>
          <div class="section-title">标签</div>
          <div v-if="item.tags?.length" class="chips">
            <span v-for="tag in item.tags" :key="tag" class="chip">{{ tag }}</span>
          </div>
          <span v-else class="ro-empty">—</span>
        </section>

        <section>
          <div class="section-title">分类</div>
          <div v-if="item.categories?.length" class="chips">
            <span v-for="category in item.categories" :key="category" class="chip">{{ category }}</span>
          </div>
          <span v-else class="ro-empty">—</span>
        </section>

        <section>
          <div class="section-title">文件夹</div>
          <span class="ro-text">{{ item.folders?.[0] || '—' }}</span>
        </section>

        <section>
          <div class="section-title">基本信息</div>
          <dl class="info">
            <dt>评分</dt>
            <dd>{{ Number(item.star) > 0 ? '★'.repeat(Number(item.star)) : '—' }}</dd>
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
          <div v-for="path in item.paths" :key="path" class="path">{{ path }}</div>
          <span v-if="!item.paths?.length" class="ro-empty">—</span>
        </section>
      </div>

      <div v-else-if="!store.viewerMode" class="fields">
        <textarea
          ref="nameArea"
          v-model="name"
          class="name-input"
          rows="1"
          title="重命名文件（回车提交）"
          @keydown.enter.prevent="($event.target as HTMLTextAreaElement).blur()"
          @blur="submitName"
        ></textarea>

        <textarea
          ref="annotationArea"
          v-model="annotation"
          rows="1"
          placeholder="添加注释（回车换行，失焦提交）"
          @keydown.ctrl.enter.prevent="($event.target as HTMLTextAreaElement).blur()"
          @blur="submitAnnotation"
        ></textarea>

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
          <select
            class="folder-select"
            :value="item.folders?.[0] ?? ''"
            title="所在文件夹（选择即移动）"
            @change="moveToFolder(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="f in folderOptions" :key="f.path" :value="f.path">{{ f.label }}</option>
          </select>
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

    <!-- 多选：批量操作（参考 Eagle 多选面板）；只读查看下隐藏全部写操作，仅保留堆叠预览与基本信息 -->
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
          <dd><StarRating v-if="!store.viewerMode" :model-value="0" @update:model-value="applyStarToAll" /><span v-else>—</span></dd>
          <dt>文件大小</dt>
          <dd>{{ formatSize(totalSelectedSize) }}</dd>
        </dl>
      </section>

      <button v-if="!store.viewerMode && !store.isTrash" class="danger" @click="store.trashSelected()">移入回收站</button>
      <button v-else-if="!store.viewerMode" @click="store.restoreSelected()">恢复</button>
      </template>
    </div>

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

    <CategoryPickerDialog v-if="showCategoryPicker" title="添加到分类" @confirm="batchAddCategory" @cancel="showCategoryPicker = false" />
    <FolderPickerDialog v-if="showFolderPicker" title="移动到文件夹" @confirm="batchMoveFolder" @cancel="showFolderPicker = false" />
  </aside>
</template>

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

.palette {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 10px 12px 0;
}

.swatch {
  width: 18px;
  height: 18px;
  padding: 0;
  border: 1px solid rgba(255, 255, 255, 0.15);
  border-radius: 50%;
  cursor: pointer;
}

.swatch.active {
  box-shadow: 0 0 0 2px var(--bg-2), 0 0 0 4px var(--accent);
}

.fields {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 12px;
}

.fields > input,
.fields > textarea {
  padding: 5px 8px;
}

.fields > textarea {
  resize: none;
  overflow: hidden;
  line-height: 1.45;
  overflow-wrap: break-word;
}

/* 触屏只读视图：与编辑版同结构，全部静态文本 */
.ro-name {
  font-weight: 600;
  word-break: break-all;
}

.ro-annotation {
  font-size: 12px;
  color: var(--fg-1);
  white-space: pre-wrap;
  word-break: break-word;
}

.ro-url {
  font-size: 12px;
  color: var(--accent);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.ro-text,
.ro-empty {
  font-size: 12px;
}

.ro-empty {
  color: var(--fg-1);
}

.name-input {
  font-weight: 600;
  word-break: break-all;
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

/* 文件夹为单值排他语义：下拉框（Eagle 同款），而非标签/分类的 chip */
.folder-select {
  width: 100%;
  padding: 6px 8px;
  border: 1px solid var(--border);
  border-radius: 5px;
  background: var(--bg-3);
  color: var(--fg-0);
  font-size: 12px;
}

.folder-select:focus {
  outline: none;
  border-color: var(--accent);
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

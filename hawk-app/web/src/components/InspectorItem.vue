<script setup lang="ts">
// 检查器单选区：完整编辑（布局参考 Eagle）。编辑字段为本地副本，切换选中项时重置；失焦/回车提交。
// 只读的两个来源：触屏设备（编辑控件易误触）与只读查看（局域网 viewer token）——同结构全静态展示。
import { computed, nextTick, ref, watch } from 'vue';
import { api } from '../api/endpoints';
import { useLibraryStore } from '../stores/library';
import { useLayout } from '../composables/useLayout';
import { formatSize, formatTime } from '../format';
import TagEditor from './TagEditor.vue';
import StarRating from './StarRating.vue';
import Icon from './Icon.vue';
import CategoryPickerDialog from './CategoryPickerDialog.vue';
import FolderTreePicker from './FolderTreePicker.vue';
import type { ViewState } from '../types';

const store = useLibraryStore();
const { touch } = useLayout();
// 只读态：触屏（iPad/手机网页，实际生效于 wide 布局的 iPad 横屏；narrow 下检查器隐藏）或
// 只读查看（viewer token）——信息结构与桌面版一致但全部静态展示，无编辑控件
const readOnly = computed(() => touch.value || store.viewerMode);
const showCategoryPicker = ref(false);

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

/** 添加到分类（＋ 按钮确认后）：复用批量追加 action（单选即长度为 1 的批量） */
function addCategory(category: string) {
  showCategoryPicker.value = false;
  store.addCategoryToSelected(category);
}

// ---- 文件夹 ----

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

function moveToFolder(path: string) {
  if (item.value && path !== (item.value.folders?.[0] ?? '')) {
    void store.updateItem(item.value.id, { folder_path: path });
  }
}

// ---- 视图跳转与颜色检索 ----

/** 点击信息（标签/分类/文件夹/文件位置）跳转对应视图；已在该视图时不重查 */
function goView(v: ViewState) {
  // 根目录素材 ⊆ 全部素材：在 all/root 视图里点根目录文件的文件夹信息不跳——
  // 两视图内容几乎一致，跳了像界面被重置
  if (v.kind === 'root' && (store.view.kind === 'all' || store.view.kind === 'root')) {
    return;
  }
  if (JSON.stringify(store.view) !== JSON.stringify(v)) {
    store.setView(v);
  }
}

/** 文件夹值（folders[0]，已是文件夹路径）→ 文件夹视图；根目录（""）→ 根目录素材视图 */
function folderViewOf(path: string): ViewState {
  return path ? { kind: 'folder', path } : { kind: 'root' };
}

/** 文件位置（完整文件路径）→ 所在文件夹视图（取目录部分；根目录文件 → 根目录素材视图） */
function parentFolderViewOf(filePath: string): ViewState {
  const dir = filePath.includes('/') ? filePath.slice(0, filePath.lastIndexOf('/')) : '';
  return dir ? { kind: 'folder', path: dir } : { kind: 'root' };
}

/** 点击色块：在当前视图范围内按颜色检索；再次点击当前检索色则清除 */
function searchColor(color: string) {
  store.setQuery({ color: store.query.color === color ? undefined : color });
}
</script>

<template>
  <template v-if="item">
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

    <!-- 只读态（触屏/只读查看）：与桌面版同样的信息结构，全部静态展示，不可修改 -->
    <div v-if="readOnly" class="fields ro-fields">
      <div class="ro-name">{{ item.name }}</div>
      <div v-if="item.annotation" class="ro-annotation">{{ item.annotation }}</div>
      <a v-if="item.url" class="ro-url" :href="item.url" target="_blank" rel="noreferrer">{{ item.url }}</a>

      <section>
        <div class="section-title">标签</div>
        <div v-if="item.tags?.length" class="chips">
          <span v-for="tag in item.tags" :key="tag" class="chip">
            <button class="jump" :title="`查看标签「${tag}」`" @click="goView({ kind: 'tag', name: tag })">{{ tag }}</button>
          </span>
        </div>
        <span v-else class="ro-empty">—</span>
      </section>

      <section>
        <div class="section-title">分类</div>
        <div v-if="item.categories?.length" class="chips">
          <span v-for="category in item.categories" :key="category" class="chip">
            <button class="jump" :title="`查看分类「${category}」`" @click="goView({ kind: 'category', name: category })">{{ category }}</button>
          </span>
        </div>
        <span v-else class="ro-empty">—</span>
      </section>

      <section>
        <div class="section-title">文件夹</div>
        <button v-if="item.folders?.[0]" class="jump ro-text" @click="goView(folderViewOf(item.folders[0]))">{{ item.folders[0] }}</button>
        <span v-else class="ro-text">—</span>
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
        <button v-for="path in item.paths" :key="path" class="path jump" :title="`查看所在文件夹：${path}`" @click="goView(parentFolderViewOf(path))">{{ path }}</button>
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
            <button class="jump" :title="`查看分类「${category}」`" @click="goView({ kind: 'category', name: category })">{{ category }}</button>
            <button class="remove" title="移出该分类" @click="removeCategory(category)">×</button>
          </span>
          <button class="add" title="添加到分类" @click="showCategoryPicker = true">＋</button>
        </div>
      </section>

      <section>
        <div class="section-title">文件夹</div>
        <div class="folder-row">
          <!-- Eagle 式：点击当前值弹出可折叠文件夹树，选择即移动（FolderTreePicker） -->
          <button ref="folderValueEl" class="folder-value" title="选择所在文件夹（点击移动）" @click="toggleFolderPicker">
            {{ item.folders?.[0] || '（根目录）' }}
          </button>
          <button class="finder" title="打开所在文件夹" @click="goView(folderViewOf(item.folders?.[0] ?? ''))">›</button>
        </div>
        <FolderTreePicker
          v-if="pickerAnchor"
          :current="item.folders?.[0] ?? ''"
          :trigger="folderValueEl"
          :anchor="pickerAnchor"
          @pick="moveToFolder"
          @close="pickerAnchor = null"
        />
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
          <button class="path jump" :title="`查看所在文件夹：${path}`" @click="goView(parentFolderViewOf(path))">{{ path }}</button>
          <!-- 多位置素材：按位置删除（其余位置保留，最后一个库内位置被删时整项回收） -->
          <button
            v-if="!store.viewerMode && (item.paths?.length ?? 0) > 1"
            class="finder danger-btn"
            title="删除此位置（其余位置保留）"
            @click="store.deleteLocation(item.id, path)"
          >
            <Icon name="trash" :size="13" />
          </button>
        </div>
      </section>
    </div>

    <CategoryPickerDialog v-if="showCategoryPicker" title="添加到分类" @confirm="addCategory" @cancel="showCategoryPicker = false" />
  </template>
</template>

<style src="./inspector-shared.css"></style>

<style scoped>
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

.folder-row {
  display: flex;
  align-items: center;
  gap: 6px;
}

.folder-row .finder {
  font-size: 16px;
}

/* 文件夹为单值排他语义：Eagle 式树选择（点击当前值弹出 FolderTreePicker，选择即移动） */
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

.folder-value:hover {
  border-color: var(--accent);
}
</style>

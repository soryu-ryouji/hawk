<script setup lang="ts">
// 单图查找：独立弹窗承载相似度调节与分页结果，不改变主列表视图和浏览历史。
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { api } from '@/shared/api/endpoints';
import { ApiError } from '@/shared/api/client';
import { errorText } from '@/shared/lib/storeUtil';
import { useContextMenu } from '@/shared/composables/useContextMenu';
import { useLibraryStore } from '../store';
import { loadText, saveText, STORAGE_KEYS } from '@/shared/lib/persist';
import Icon from '@/shared/ui/Icon.vue';
import type { Item, VisionStatus } from '@/shared/types';

const props = defineProps<{ source: Item }>();
const emit = defineEmits<{ close: [] }>();
const store = useLibraryStore();
const menu = useContextMenu();

const PAGE_SIZE = 60;
const searchMode = ref<'hash' | 'composition'>('hash');
const visionStatus = ref<VisionStatus | null>(null);
const visionScores = ref(new Map<string, number>());
const thumbFailed = ref(new Set<string>());
const thumbAttempts = ref(new Map<string, number>());
const thumbTimers = new Map<string, ReturnType<typeof setTimeout>>();
// 0–100 映射距离 20–0；默认中间值 50，用户设置在 web/Electron 持久化
function initialSimilarity(): number {
  const key = searchMode.value === 'composition' ? STORAGE_KEYS.visionSimilarityThreshold : STORAGE_KEYS.similarityThreshold;
  const fallback = searchMode.value === 'composition' ? 70 : 50;
  const raw = loadText(key);
  if (raw === null) {
    return fallback;
  }
  const value = Number(raw);
  return Number.isFinite(value) ? Math.min(100, Math.max(0, Math.round(value))) : fallback;
}
const similarity = ref(initialSimilarity());
const items = ref<Item[]>([]);
const total = ref(0);
const offset = ref(0);
const loading = ref(false);
const error = ref('');
const closeButton = ref<HTMLButtonElement | null>(null);
let scanVersion = 0;
let statusTimer: ReturnType<typeof setTimeout> | undefined;
let notReadyAttempts = 0;

const maxDistance = computed(() => Math.round((100 - similarity.value) / 5));
const totalPages = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)));
const currentPage = computed(() => Math.min(totalPages.value, Math.floor(offset.value / PAGE_SIZE) + 1));

onMounted(() => {
  closeButton.value?.focus();
  void refreshVisionStatus();
});
onUnmounted(() => {
  clearTimeout(statusTimer);
  for (const timer of thumbTimers.values()) {
    clearTimeout(timer);
  }
});

function thumbSrc(item: Item): string {
  const base = api.thumbnailUrl(item.id, 512);
  const attempt = thumbAttempts.value.get(item.id) ?? 0;
  return attempt > 0 ? `${base}&retry=${attempt}` : base;
}

function onThumbError(item: Item) {
  const attempt = (thumbAttempts.value.get(item.id) ?? 0) + 1;
  thumbAttempts.value.set(item.id, attempt);
  thumbFailed.value.add(item.id);
  if (attempt > 6) {
    return;
  }
  thumbTimers.set(
    item.id,
    setTimeout(
      () => {
        thumbFailed.value.delete(item.id);
      },
      Math.min(500 * 2 ** (attempt - 1), 5000),
    ),
  );
}

function onThumbLoad(item: Item) {
  thumbFailed.value.delete(item.id);
  thumbAttempts.value.delete(item.id);
  const timer = thumbTimers.get(item.id);
  if (timer) {
    clearTimeout(timer);
    thumbTimers.delete(item.id);
  }
}

async function refreshVisionStatus() {
  const hadStatus = visionStatus.value !== null;
  const wasEnabled = visionStatus.value?.enabled ?? false;
  const wasIndexing = visionStatus.value?.indexing ?? false;
  const previousPending = visionStatus.value?.pending_items ?? 0;
  visionStatus.value = await api.visionStatus().catch(() => null);
  const current = visionStatus.value;
  if (!current) {
    return;
  }
  if (hadStatus && current.enabled && (!wasEnabled || (wasIndexing && !current.indexing) || (previousPending > 0 && current.pending_items === 0))) {
    offset.value = 0;
    void scan();
  }
  if (current.downloading || current.indexing || current.pending_items > 0) {
    clearTimeout(statusTimer);
    statusTimer = setTimeout(() => void refreshVisionStatus(), 700);
  }
}

watch(
  () => [props.source.id, props.source.path],
  () => {
    notReadyAttempts = 0;
    offset.value = 0;
    void scan();
  },
  { immediate: true },
);

function setSearchMode(mode: 'hash' | 'composition') {
  notReadyAttempts = 0;
  searchMode.value = mode;
  similarity.value = initialSimilarity();
  offset.value = 0;
  void scan();
}

function onThresholdChange() {
  // 拖动只更新相似度，松开/键盘提交后才持久化并重新查询
  notReadyAttempts = 0;
  const key = searchMode.value === 'composition' ? STORAGE_KEYS.visionSimilarityThreshold : STORAGE_KEYS.similarityThreshold;
  saveText(key, String(similarity.value));
  offset.value = 0;
  void scan();
}

async function installVision() {
  visionStatus.value = await api.visionInstall().catch((e: unknown) => {
    error.value = errorText(e);
    return null;
  });
  if (visionStatus.value) {
    await refreshVisionStatus();
  }
}

async function enableVision() {
  visionStatus.value = await api.visionEnable(true).catch((e: unknown) => {
    error.value = errorText(e);
    return null;
  });
  if (visionStatus.value) {
    await refreshVisionStatus();
  }
}

async function scan() {
  const version = ++scanVersion;
  loading.value = true;
  error.value = '';
  items.value = [];
  visionScores.value = new Map();
  thumbFailed.value = new Set();
  thumbAttempts.value = new Map();
  for (const timer of thumbTimers.values()) {
    clearTimeout(timer);
  }
  thumbTimers.clear();
  if (searchMode.value === 'composition' && (!visionStatus.value?.installed || !visionStatus.value.enabled)) {
    total.value = 0;
    loading.value = false;
    return;
  }
  try {
    if (searchMode.value === 'composition') {
      const response = await api.visionSearch({
        id: props.source.id,
        min_similarity: similarity.value / 100,
        offset: offset.value,
        limit: PAGE_SIZE,
      });
      if (version !== scanVersion) {
        return;
      }
      items.value = response.items.map((entry) => entry.item);
      visionScores.value = new Map(response.items.map((entry) => [entry.item.id, entry.similarity_score]));
      total.value = Number(response.total);
    } else {
      const response = await api.itemList({
        similar_to: props.source.id,
        max_distance: maxDistance.value,
        order_by: 'similarity',
        order: 'asc',
        offset: offset.value,
        limit: PAGE_SIZE,
      });
      if (version !== scanVersion) {
        return;
      }
      items.value = response.items;
      total.value = Number(response.total);
    }
    notReadyAttempts = 0;
  } catch (e) {
    if (version === scanVersion && e instanceof ApiError && e.code === 'NOT_READY' && notReadyAttempts < 6) {
      notReadyAttempts++;
      clearTimeout(statusTimer);
      statusTimer = setTimeout(() => {
        void refreshVisionStatus();
        void scan();
      }, 1200);
    } else if (version === scanVersion) {
      error.value = visionStatus.value?.error ?? errorText(e);
    }
  } finally {
    if (version === scanVersion) {
      loading.value = false;
    }
  }
}

function pageBy(step: number) {
  offset.value = Math.max(0, Math.min((totalPages.value - 1) * PAGE_SIZE, offset.value + step * PAGE_SIZE));
  void scan();
}

function onItemMenu(item: Item, event: MouseEvent) {
  menu.open(
    [
      {
        label: '跳转到所在文件夹',
        action: () => {
          const index = item.path.lastIndexOf('/');
          const path = index < 0 ? '' : item.path.slice(0, index);
          emit('close');
          store.setView({ kind: 'folder', path });
        },
      },
    ],
    event,
  );
}
</script>

<template>
  <Teleport to="body">
    <div class="mask" @click.self="emit('close')">
      <section class="dialog" role="dialog" aria-modal="true" aria-label="查找图片" @keydown.stop @keydown.esc="emit('close')">
        <header class="head">
          <div>
            <h2>查找图片</h2>
            <p>调整相似度要求，查找改名、缩放、重压缩或视觉相近的图片。</p>
          </div>
          <button ref="closeButton" type="button" class="icon-btn" title="关闭 (Esc)" @click="emit('close')"><Icon name="close" :size="16" /></button>
        </header>

        <div class="search-modes" role="tablist" aria-label="查找方式">
          <button type="button" role="tab" :aria-selected="searchMode === 'hash'" :class="{ active: searchMode === 'hash' }" @click="setSearchMode('hash')">
            相同图片
          </button>
          <button
            type="button"
            role="tab"
            :aria-selected="searchMode === 'composition'"
            :class="{ active: searchMode === 'composition' }"
            @click="setSearchMode('composition')"
          >
            相似构图
          </button>
        </div>

        <div class="source-row">
          <div v-if="thumbFailed.has(source.id)" class="thumb-placeholder source-thumb">
            {{ (thumbAttempts.get(source.id) ?? 0) > 6 ? '缩略图不可用' : '缩略图生成中…' }}
          </div>
          <img v-else class="source-thumb" :src="thumbSrc(source)" :alt="source.name" @error="onThumbError(source)" @load="onThumbLoad(source)" />
          <div class="source-copy">
            <span class="eyebrow">源图片</span>
            <strong :title="`${source.name}.${source.ext}`">{{ source.name }}.{{ source.ext }}</strong>
            <span>{{ source.width }} × {{ source.height }}</span>
          </div>
        </div>

        <div class="threshold-row">
          <span class="threshold-label"
            >相似度要求 <strong>{{ similarity }}</strong></span
          >
          <input v-model.number="similarity" type="range" min="0" max="100" step="1" aria-label="相似度要求" @change="onThresholdChange" />
          <span class="threshold-scale"><span>宽松</span><span>严格</span></span>
        </div>

        <div class="result-meta">
          <span v-if="!error">共 {{ total }} 项 · 按{{ searchMode === 'composition' ? '相似度' : '差异' }}排序</span>
          <span v-else>查询失败</span>
          <span v-if="searchMode === 'hash'">差异范围 0–{{ maxDistance }}</span>
          <span v-else>余弦相似度 ≥ {{ similarity }}</span>
        </div>

        <div v-if="searchMode === 'composition' && (visionStatus?.indexing || visionStatus?.pending_items)" class="index-hint">
          正在建立构图索引：{{ visionStatus.indexed_items }} / {{ visionStatus.total_items }}，新图会在后台自动补齐
        </div>
        <div class="results">
          <div v-if="searchMode === 'composition' && !visionStatus?.installed" class="state">
            <p>相似构图需要可选视觉扩展，模型约 24 MB，只在下载时联网。</p>
            <button type="button" :disabled="visionStatus?.downloading" @click="installVision">
              {{ visionStatus?.downloading ? '正在下载…' : '下载并启用视觉扩展' }}
            </button>
          </div>
          <div v-else-if="searchMode === 'composition' && !visionStatus?.enabled" class="state">
            <p>视觉扩展已安装但未启用。</p>
            <button type="button" @click="enableVision">启用视觉扩展</button>
          </div>
          <div v-else-if="loading" class="state">正在查找图片…</div>
          <div v-else-if="error" class="state error">
            <p>{{ error }}</p>
            <button type="button" @click="scan">重试</button>
          </div>
          <div v-else-if="items.length === 0" class="state">未找到匹配图片。</div>
          <div v-else class="gallery">
            <article v-for="item in items" :key="`${item.id}\n${item.path}`" class="result-card" @contextmenu.prevent="onItemMenu(item, $event)">
              <div v-if="thumbFailed.has(item.id)" class="thumb-placeholder result-thumb">
                {{ (thumbAttempts.get(item.id) ?? 0) > 6 ? '缩略图不可用' : '缩略图生成中…' }}
              </div>
              <img v-else class="result-thumb" :src="thumbSrc(item)" :alt="item.name" loading="lazy" @error="onThumbError(item)" @load="onThumbLoad(item)" />
              <div class="result-copy">
                <span class="result-name" :title="`${item.name}.${item.ext}`">{{ item.name }}.{{ item.ext }}</span>
                <span class="result-info">
                  {{ item.width }} × {{ item.height }} ·
                  <template v-if="searchMode === 'composition'">相似度 {{ Math.round((visionScores.get(item.id) ?? 0) * 100) }}%</template>
                  <template v-else>差异 {{ item.similarity_distance ?? '—' }}</template>
                </span>
              </div>
            </article>
          </div>
        </div>

        <footer v-if="totalPages > 1" class="footer">
          <button type="button" :disabled="offset === 0 || loading" @click="pageBy(-1)">上一页</button>
          <span>第 {{ currentPage }} / {{ totalPages }} 页</span>
          <button type="button" :disabled="currentPage >= totalPages || loading" @click="pageBy(1)">下一页</button>
        </footer>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 260;
  display: grid;
  place-items: center;
  padding: 24px;
  background: rgba(0, 0, 0, 0.62);
  backdrop-filter: blur(6px);
}

.dialog {
  display: flex;
  flex-direction: column;
  width: min(1080px, 100%);
  height: min(800px, 100%);
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--bg-1);
  box-shadow: 0 20px 60px rgba(0, 0, 0, 0.45);
}

.head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  padding: 18px 20px 14px;
  border-bottom: 1px solid var(--border);
}

.head h2 {
  color: var(--fg-0);
  font-size: 16px;
  font-weight: 600;
}

.head p {
  margin-top: 5px;
  color: var(--fg-1);
  font-size: 12px;
}

.icon-btn {
  display: grid;
  place-items: center;
  width: 28px;
  height: 28px;
  flex: none;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--fg-1);
}

.source-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 20px;
}

.source-thumb {
  width: 52px;
  height: 52px;
  flex: none;
  object-fit: contain;
  border-radius: 6px;
  background: var(--bg-0);
}

.source-copy {
  display: flex;
  min-width: 0;
  flex: 1;
  flex-direction: column;
  gap: 3px;
  color: var(--fg-1);
  font-size: 11px;
}

.source-copy strong {
  overflow: hidden;
  color: var(--fg-0);
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.eyebrow {
  font-size: 10px;
}

.threshold-row {
  display: grid;
  grid-template-columns: 120px minmax(120px, 1fr) 70px;
  align-items: center;
  gap: 12px;
  padding: 0 20px 12px;
  color: var(--fg-1);
  font-size: 11px;
}

.threshold-label strong {
  margin-left: 4px;
  color: var(--fg-0);
  font-weight: 500;
}

.threshold-row input {
  width: 100%;
  padding: 0;
  accent-color: var(--accent);
}

.threshold-scale {
  display: flex;
  justify-content: space-between;
  gap: 5px;
  font-size: 10px;
}

.search-modes {
  display: flex;
  gap: 4px;
  padding: 10px 20px 0;
}

.search-modes button {
  padding: 5px 12px;
  border: none;
  background: transparent;
  color: var(--fg-1);
  font-size: 12px;
}

.search-modes button.active {
  background: var(--bg-3);
  color: var(--fg-0);
}

.index-hint {
  padding: 0 20px 10px;
  color: var(--fg-1);
  font-size: 11px;
}

.result-meta {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  padding: 0 20px 10px;
  color: var(--fg-1);
  font-size: 11px;
}

.results {
  min-height: 0;
  flex: 1;
  overflow-y: auto;
  border-top: 1px solid var(--border);
  background: var(--bg-0);
}

.gallery {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 12px;
  padding: 16px 20px;
}

.result-card {
  min-width: 0;
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: 7px;
  background: var(--bg-2);
}

.result-thumb {
  display: block;
  width: 100%;
  aspect-ratio: 1;
  object-fit: contain;
  background: var(--bg-0);
}

.thumb-placeholder {
  display: grid;
  place-items: center;
  color: var(--fg-1);
  font-size: 11px;
  text-align: center;
}

.result-copy {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px 10px;
}

.result-name {
  overflow: hidden;
  color: var(--fg-0);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.result-info {
  color: var(--fg-1);
  font-size: 10px;
}

.state {
  display: grid;
  min-height: 240px;
  place-content: center;
  justify-items: center;
  gap: 12px;
  color: var(--fg-1);
  text-align: center;
}

.state.error {
  color: var(--danger);
}

.footer {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  padding: 10px 20px;
  border-top: 1px solid var(--border);
  color: var(--fg-1);
  font-size: 12px;
}

.footer button {
  padding: 4px 10px;
}

@media (max-width: 640px) {
  .mask {
    padding: 8px;
  }

  .head,
  .source-row {
    padding-right: 12px;
    padding-left: 12px;
  }

  .source-row {
    flex-wrap: wrap;
  }

  .gallery {
    grid-template-columns: repeat(auto-fill, minmax(110px, 1fr));
    padding: 12px;
  }
}
</style>

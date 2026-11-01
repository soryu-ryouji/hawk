<script setup lang="ts">
// 重复图片管理：扫描（只读）→ 用户逐位置选择 → 二次确认 → 按位置移入回收站。
// 默认不选中、不自动保留“最佳版本”，不做永久删除。
import { computed, ref } from 'vue';
import { api } from '@/shared/api/endpoints';
import { errorText } from '@/shared/lib/storeUtil';
import { formatSize } from '@/shared/lib/format';
import ConfirmDialog from '@/shared/ui/ConfirmDialog.vue';
import { itemKey, splitKey } from '@/domains/library';
import { useLibraryStore } from '@/domains/library';
import type { DuplicateGroup } from '@/shared/types';

type Kind = 'similar' | 'exact';

const PAGE_SIZE = 50;
const store = useLibraryStore();
const kind = ref<Kind>('similar');
// Eagle 式相似度：0 最宽松（距离 20），100 最严格（距离 0）；默认 75 对应原严格档 5
const similarity = ref(75);
const maxDistance = computed(() => Math.round((100 - similarity.value) / 5));
const groups = ref<DuplicateGroup[]>([]);
const total = ref(0);
const offset = ref(0);
const scanned = ref(false);
const loading = ref(false);
const error = ref('');
const result = ref('');
const selected = ref(new Set<string>());
const confirmOpen = ref(false);
const deleting = ref(false);
let scanVersion = 0;

const selectedCount = computed(() => selected.value.size);
const totalPages = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)));
const currentPage = computed(() => Math.min(totalPages.value, Math.floor(offset.value / PAGE_SIZE) + 1));
const actionText = computed(() => (loading.value ? '扫描中…' : scanned.value ? '重新扫描' : '开始扫描'));

function switchKind(next: Kind) {
  if (kind.value === next) {
    return;
  }
  scanVersion++;
  kind.value = next;
  groups.value = [];
  total.value = 0;
  offset.value = 0;
  selected.value = new Set();
  scanned.value = false;
  error.value = '';
  result.value = '';
}

function onThresholdChange() {
  // range 的 input 只更新数值，change（拖动松开/键盘提交）才重扫，避免连续发起全库计算
  if (scanned.value) {
    void startScan();
  }
}

async function startScan() {
  scanVersion++;
  offset.value = 0;
  selected.value = new Set();
  groups.value = [];
  scanned.value = false;
  result.value = '';
  await scan();
}

async function scan() {
  const version = ++scanVersion;
  loading.value = true;
  error.value = '';
  try {
    const response = await api.itemDuplicates({
      kind: kind.value,
      max_distance: kind.value === 'similar' ? maxDistance.value : undefined,
      offset: offset.value,
      limit: PAGE_SIZE,
    });
    if (version !== scanVersion) {
      return;
    }
    groups.value = response.groups;
    total.value = Number(response.total);
    scanned.value = true;
    // 删除导致当前页越界时退回有效最后一页；选择仍保留（失败项需要重试）
    if (offset.value >= total.value && offset.value > 0) {
      offset.value = Math.floor(Math.max(0, total.value - 1) / PAGE_SIZE) * PAGE_SIZE;
      await scan();
    }
  } catch (e) {
    if (version === scanVersion) {
      error.value = errorText(e);
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

function toggleEntry(id: string, path: string) {
  const key = itemKey(id, path);
  const next = new Set(selected.value);
  if (next.has(key)) {
    next.delete(key);
  } else {
    next.add(key);
  }
  selected.value = next;
}

function folderOf(path: string): string {
  const index = path.lastIndexOf('/');
  return index < 0 ? '（根目录）' : path.slice(0, index);
}

async function deleteSelected() {
  if (selectedCount.value === 0 || deleting.value) {
    return;
  }
  deleting.value = true;
  result.value = '';
  let succeeded = 0;
  let failed = 0;
  for (const key of [...selected.value]) {
    const { id, path } = splitKey(key);
    try {
      await api.itemDelete(id, path);
      const next = new Set(selected.value);
      next.delete(key);
      selected.value = next;
      succeeded++;
    } catch {
      failed++;
    }
  }
  confirmOpen.value = false;
  deleting.value = false;
  result.value = failed > 0 ? `已移入回收站 ${succeeded} 个位置，${failed} 个失败（仍保持选中，可重试）` : `已将 ${succeeded} 个文件位置移入回收站`;
  store.showToast(result.value);
  await scan();
}
</script>

<template>
  <section class="settings-section duplicate-manager">
    <h3 class="settings-section-title">重复图片</h3>
    <div class="settings-group">
      <div class="duplicate-head">
        <div class="duplicate-tabs" role="tablist">
          <button type="button" :class="{ active: kind === 'similar' }" :disabled="loading" @click="switchKind('similar')">相似图片</button>
          <button type="button" :class="{ active: kind === 'exact' }" :disabled="loading" @click="switchKind('exact')">相同内容</button>
        </div>
        <div v-if="kind === 'similar'" class="threshold">
          <div class="threshold-label">
            <span>相似度</span>
            <span>{{ similarity }}</span>
          </div>
          <input
            v-model.number="similarity"
            type="range"
            min="0"
            max="100"
            step="1"
            aria-label="相似度"
            :title="`向右提高相似度要求（当前差异 0–${maxDistance}）`"
            :disabled="loading || deleting"
            @change="onThresholdChange"
          />
          <div class="threshold-scale"><span>宽松</span><span>严格</span></div>
        </div>
        <button type="button" class="scan-button" :disabled="loading || deleting" @click="startScan">{{ actionText }}</button>
      </div>

      <p class="settings-message">
        {{
          kind === 'similar'
            ? '按感知哈希分组。相似不等于重复，请逐张确认；不会自动删除任何文件。'
            : '检查同一 hash 下的多个文件位置，只列出库内位置数大于 1 的组。'
        }}
      </p>
      <p v-if="error" class="settings-message error">{{ error }}</p>
      <p v-if="result" class="settings-message" :class="{ error: result.includes('失败') }">{{ result }}</p>
      <p v-if="loading && groups.length === 0" class="settings-message">正在扫描重复图片…</p>
      <p v-else-if="!scanned" class="settings-message">选择类型后开始扫描，所有图片默认不选中。</p>
      <p v-else-if="total === 0" class="settings-message">当前条件下未发现重复图片。</p>

      <div v-if="groups.length > 0" class="duplicate-results">
        <section v-for="group in groups" :key="group.id" class="duplicate-group">
          <header class="group-head">
            <span>{{ group.kind === 'exact' ? '相同内容' : '相似图片' }} · {{ group.entries.length }} 个位置</span>
            <span class="group-id">#{{ group.id.slice(0, 8) }}</span>
          </header>
          <label v-for="entry in group.entries" :key="itemKey(entry.item.id, entry.item.path)" class="duplicate-entry">
            <input
              type="checkbox"
              :checked="selected.has(itemKey(entry.item.id, entry.item.path))"
              :disabled="deleting"
              @change="toggleEntry(entry.item.id, entry.item.path)"
            />
            <img class="duplicate-thumb" :src="api.thumbnailUrl(entry.item.id, 512)" :alt="entry.item.name" loading="lazy" />
            <span class="entry-copy">
              <span class="entry-name">{{ entry.item.name }}.{{ entry.item.ext }}</span>
              <span class="entry-meta">
                {{ formatSize(Number(entry.item.size)) }} · {{ entry.item.width }} × {{ entry.item.height }}
                <template v-if="entry.similarity_distance !== undefined && entry.similarity_distance !== null">
                  · 差异 {{ entry.similarity_distance }}
                </template>
              </span>
              <span class="entry-folder">文件夹：{{ folderOf(entry.item.path) }}</span>
              <span class="entry-path" :title="entry.item.path">{{ entry.item.path }}</span>
            </span>
          </label>
        </section>
      </div>

      <div v-if="total > 0" class="duplicate-footer">
        <div class="pagination">
          <button type="button" :disabled="offset === 0 || loading" @click="pageBy(-1)">上一页</button>
          <span>第 {{ currentPage }} / {{ totalPages }} 页，共 {{ total }} 组</span>
          <button type="button" :disabled="currentPage >= totalPages || loading" @click="pageBy(1)">下一页</button>
        </div>
        <div class="duplicate-actions">
          <span>已选择 {{ selectedCount }} 个文件位置</span>
          <button type="button" class="danger" :disabled="selectedCount === 0 || deleting" @click="confirmOpen = true">
            {{ deleting ? '正在移入回收站…' : '移入回收站' }}
          </button>
        </div>
      </div>
    </div>

    <ConfirmDialog
      v-if="confirmOpen"
      title="移入回收站？"
      :description="`将移动 ${selectedCount} 个文件位置。其他位置不会被删除；同一 item 的最后一个库内位置被移动后，该 item 也会进入回收站，之后仍可恢复。`"
      confirm-text="确认移入回收站"
      danger
      @confirm="deleteSelected"
      @cancel="confirmOpen = false"
    />
  </section>
</template>

<style scoped>
.duplicate-head,
.duplicate-tabs,
.duplicate-footer,
.pagination,
.duplicate-actions {
  display: flex;
  align-items: center;
}

.duplicate-head {
  gap: 8px;
  flex-wrap: wrap;
  padding: 10px 12px;
}

.duplicate-tabs {
  gap: 2px;
  padding: 2px;
  border-radius: 7px;
  background: var(--bg-1);
}

.duplicate-tabs button,
.scan-button,
.pagination button,
.duplicate-actions button {
  padding: 5px 10px;
  border-radius: 6px;
  color: var(--fg-1);
  font-size: 12px;
}

.duplicate-tabs button.active {
  background: var(--bg-3);
  color: var(--fg-0);
}

.threshold {
  width: min(210px, 100%);
  color: var(--fg-1);
  font-size: 11px;
}

.threshold-label,
.threshold-scale {
  display: flex;
  justify-content: space-between;
  gap: 8px;
}

.threshold-label {
  color: var(--fg-0);
  font-size: 12px;
}

.threshold input {
  width: 100%;
  height: 16px;
  padding: 0;
  accent-color: var(--accent);
}

.threshold-scale {
  color: var(--fg-1);
  font-size: 10px;
}

.scan-button {
  margin-left: auto;
  border: 1px solid var(--accent);
  background: var(--accent);
  color: #fff;
}

.duplicate-results {
  max-height: 560px;
  overflow-y: auto;
  border-top: 1px solid var(--border);
}

.duplicate-group + .duplicate-group {
  border-top: 1px solid var(--border);
}

.group-head {
  position: sticky;
  top: 0;
  z-index: 1;
  display: flex;
  justify-content: space-between;
  padding: 7px 12px;
  background: var(--bg-2);
  color: var(--fg-1);
  font-size: 11px;
}

.group-id {
  font-family: monospace;
}

.duplicate-entry {
  display: grid;
  grid-template-columns: 18px 64px minmax(0, 1fr);
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  cursor: pointer;
}

.duplicate-entry:hover {
  background: var(--bg-1);
}

.duplicate-thumb {
  width: 64px;
  height: 64px;
  object-fit: contain;
  background: var(--bg-0);
  border-radius: 5px;
}

.entry-copy {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 3px;
}

.entry-name {
  overflow: hidden;
  color: var(--fg-0);
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.entry-meta,
.entry-folder,
.entry-path {
  color: var(--fg-1);
  font-size: 11px;
}

.entry-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.duplicate-footer {
  position: sticky;
  bottom: 0;
  z-index: 2;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: 8px;
  padding: 10px 12px;
  border-top: 1px solid var(--border);
  background: var(--bg-2);
}

.pagination,
.duplicate-actions {
  gap: 8px;
  color: var(--fg-1);
  font-size: 12px;
}

.duplicate-actions button.danger {
  border: 1px solid var(--danger);
  color: var(--danger);
}
</style>

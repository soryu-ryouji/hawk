<script setup lang="ts">
// 重复图片管理：扫描（只读）→ 用户逐位置选择 → 二次确认 → 按位置移入回收站。
// 默认不选中、不自动保留“最佳版本”，不做永久删除。
import { computed, onMounted, onUnmounted, ref } from 'vue';
import { useEventListener } from '@vueuse/core';
import { api } from '@/shared/api/endpoints';
import { errorText } from '@/shared/lib/storeUtil';
import { formatSize } from '@/shared/lib/format';
import ConfirmDialog from '@/shared/ui/ConfirmDialog.vue';
import Icon from '@/shared/ui/Icon.vue';
import { itemKey, splitKey } from '@/domains/library';
import { useLibraryStore } from '@/domains/library';
import type { DuplicateGroup, Item } from '@/shared/types';

const emit = defineEmits<{ close: [] }>();
const PAGE_SIZE = 50;
const store = useLibraryStore();
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
const closeButton = ref<HTMLButtonElement | null>(null);
let scanVersion = 0;

// 缩略图是惰性缓存：首次读取可能 404 并在后台生成。这里显示占位并自动带 cache-bust 重试。
const MAX_THUMB_ATTEMPTS = 6;
const thumbFailed = ref(new Set<string>());
const thumbAttempts = ref(new Map<string, number>());
const thumbTimers = new Map<string, ReturnType<typeof setTimeout>>();

onMounted(() => {
  document.body.classList.add('duplicate-dialog-open');
  closeButton.value?.focus();
});
onUnmounted(() => {
  document.body.classList.remove('duplicate-dialog-open');
  for (const timer of thumbTimers.values()) {
    clearTimeout(timer);
  }
});

useEventListener(
  window,
  'keydown',
  (e) => {
    if (e.key !== 'Escape') {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (confirmOpen.value) {
      confirmOpen.value = false;
    } else {
      emit('close');
    }
  },
  { capture: true },
);

function thumbSrc(item: Item): string {
  const base = api.thumbnailUrl(item.id, 512);
  const attempt = thumbAttempts.value.get(item.id) ?? 0;
  return attempt > 0 ? `${base}&retry=${attempt}` : base;
}

function onThumbError(item: Item) {
  // 同一内容的多个位置共享缩略图：已有重试计时器时只保留一个，避免重复位置连续消耗次数。
  if (thumbTimers.has(item.id)) {
    return;
  }
  const attempt = (thumbAttempts.value.get(item.id) ?? 0) + 1;
  thumbAttempts.value.set(item.id, attempt);
  thumbFailed.value.add(item.id);
  if (attempt > MAX_THUMB_ATTEMPTS) {
    return;
  }
  thumbTimers.set(
    item.id,
    setTimeout(
      () => {
        thumbTimers.delete(item.id);
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

const selectedCount = computed(() => selected.value.size);
const totalPages = computed(() => Math.max(1, Math.ceil(total.value / PAGE_SIZE)));
const currentPage = computed(() => Math.min(totalPages.value, Math.floor(offset.value / PAGE_SIZE) + 1));
const actionText = computed(() => (loading.value ? '扫描中…' : scanned.value ? '重新扫描' : '开始扫描'));

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
  <Teleport to="body">
    <div class="mask" @click.self="emit('close')">
      <section class="dialog" role="dialog" aria-modal="true" aria-label="重复图片" @keydown.stop>
        <header class="dialog-head">
          <div class="dialog-title">
            <h2>重复图片</h2>
            <p>按内容 hash 汇总同一素材的多个文件位置；扫描只读，不会自动删除任何文件。</p>
          </div>
          <button ref="closeButton" type="button" class="icon-btn" title="关闭 (Esc)" @click="emit('close')">
            <Icon name="close" :size="16" />
          </button>
        </header>

        <div class="toolbar">
          <button type="button" class="scan-button" :disabled="loading || deleting" @click="startScan">{{ actionText }}</button>
          <span class="selection-count">已选择 {{ selectedCount }} 个文件位置</span>
          <button type="button" class="delete-button" :disabled="selectedCount === 0 || deleting" @click="confirmOpen = true">
            <Icon name="trash" :size="14" />
            {{ deleting ? '正在移入回收站…' : '删除选中' }}
          </button>
        </div>

        <div class="dialog-body">
          <p class="settings-message">检查同一内容 hash 下的多个文件位置，只列出库内位置数大于 1 的组；不会自动删除任何文件。</p>
          <p v-if="error" class="settings-message error">{{ error }}</p>
          <p v-if="result" class="settings-message" :class="{ error: result.includes('失败') }">{{ result }}</p>
          <p v-if="loading && groups.length === 0" class="settings-message">正在扫描重复图片…</p>
          <p v-else-if="!scanned" class="settings-message">开始扫描后，所有文件位置默认不选中。</p>
          <p v-else-if="total === 0" class="settings-message">当前条件下未发现重复图片。</p>

          <div v-if="groups.length > 0" class="duplicate-results">
            <section v-for="group in groups" :key="group.id" class="duplicate-group">
              <header class="group-head">
                <span>相同内容 · {{ group.entries.length }} 个位置</span>
                <span class="group-id">#{{ group.id.slice(0, 8) }}</span>
              </header>
              <label v-for="entry in group.entries" :key="itemKey(entry.item.id, entry.item.path)" class="duplicate-entry">
                <input
                  type="checkbox"
                  :checked="selected.has(itemKey(entry.item.id, entry.item.path))"
                  :disabled="deleting"
                  @change="toggleEntry(entry.item.id, entry.item.path)"
                />
                <div v-if="thumbFailed.has(entry.item.id)" class="duplicate-thumb thumb-placeholder">
                  {{ (thumbAttempts.get(entry.item.id) ?? 0) > MAX_THUMB_ATTEMPTS ? '无预览' : '缩略图生成中…' }}
                </div>
                <img
                  v-else
                  class="duplicate-thumb"
                  :src="thumbSrc(entry.item)"
                  :alt="entry.item.name"
                  loading="lazy"
                  @error="onThumbError(entry.item)"
                  @load="onThumbLoad(entry.item)"
                />
                <span class="entry-copy">
                  <span class="entry-name">{{ entry.item.name }}.{{ entry.item.ext }}</span>
                  <span class="entry-meta"> {{ formatSize(Number(entry.item.size)) }} · {{ entry.item.width }} × {{ entry.item.height }} </span>
                  <span class="entry-folder">文件夹：{{ folderOf(entry.item.path) }}</span>
                  <span class="entry-path" :title="entry.item.path">{{ entry.item.path }}</span>
                </span>
              </label>
            </section>
          </div>
        </div>

        <footer v-if="total > 0" class="footer">
          <button type="button" :disabled="offset === 0 || loading" @click="pageBy(-1)">上一页</button>
          <span>第 {{ currentPage }} / {{ totalPages }} 页，共 {{ total }} 组</span>
          <button type="button" :disabled="currentPage >= totalPages || loading" @click="pageBy(1)">下一页</button>
        </footer>
      </section>
    </div>
  </Teleport>

  <ConfirmDialog
    v-if="confirmOpen"
    title="移入回收站？"
    :description="`将移动 ${selectedCount} 个文件位置。其他位置不会被删除；同一 item 的最后一个库内位置被移动后，该 item 也会进入回收站，之后仍可恢复。`"
    confirm-text="确认移入回收站"
    danger
    @confirm="deleteSelected"
    @cancel="confirmOpen = false"
  />
</template>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  z-index: 180;
  display: grid;
  place-items: center;
  padding: 20px;
  background: rgba(0, 0, 0, 0.58);
  backdrop-filter: blur(5px);
}

.dialog {
  display: flex;
  width: min(1000px, 100%);
  height: min(780px, 100%);
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: 12px;
  background: var(--bg-1);
  box-shadow: 0 20px 60px rgba(0, 0, 0, 0.45);
}

.dialog-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  padding: 16px 18px 12px;
  border-bottom: 1px solid var(--border);
}

.dialog-title h2 {
  color: var(--fg-0);
  font-size: 16px;
  font-weight: 600;
}

.dialog-title p {
  margin-top: 5px;
  color: var(--fg-1);
  font-size: 12px;
}

.icon-btn {
  display: grid;
  width: 28px;
  height: 28px;
  flex: none;
  place-items: center;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--fg-1);
}

.toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 18px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-2);
}

.toolbar button,
.footer button {
  font: inherit;
  line-height: 1.2;
}

.scan-button {
  padding: 6px 12px;
  border-color: var(--accent);
  background: var(--accent);
  color: #fff;
}

.selection-count {
  margin-left: auto;
  color: var(--fg-1);
  font-size: 12px;
}

.delete-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 6px 12px;
  border: 1px solid var(--danger);
  background: var(--danger);
  color: #fff;
}

.delete-button:disabled {
  opacity: 0.45;
  cursor: default;
}

.dialog-body {
  min-height: 0;
  flex: 1;
  overflow-y: auto;
  background: var(--bg-0);
}

.settings-message {
  padding: 14px 18px 0;
  color: var(--fg-1);
  font-size: 12px;
}

.settings-message.error {
  color: var(--danger);
}

.duplicate-results {
  margin-top: 12px;
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
  padding: 7px 18px;
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
  padding: 8px 18px;
  cursor: pointer;
}

.duplicate-entry:hover {
  background: var(--bg-1);
}

.duplicate-thumb {
  width: 64px;
  height: 64px;
  object-fit: contain;
  border-radius: 5px;
  background: var(--bg-2);
}

.thumb-placeholder {
  display: grid;
  place-items: center;
  padding: 4px;
  color: var(--fg-1);
  font-size: 10px;
  text-align: center;
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

.footer {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 10px 18px;
  border-top: 1px solid var(--border);
  background: var(--bg-2);
  color: var(--fg-1);
  font-size: 12px;
}

.footer button {
  padding: 5px 10px;
  border-radius: 6px;
}
</style>

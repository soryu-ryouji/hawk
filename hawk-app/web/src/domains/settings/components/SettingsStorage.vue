<script setup lang="ts">
// 存储分区（仅 Electron）：周期重扫、全局缓存目录迁移与索引体检。
// 素材元数据固定使用 .hawk/metadata.db，不再提供 TOML/SQLite 双模式切换。
// 缓存迁移为整体搬迁（先复制后删除），期间 server 重启（前端经 serverRestarting 切启动屏）。
import { computed, onMounted, ref } from 'vue';
import { shell } from '@/shared/lib/platform';
import { useLibraryStore } from '@/domains/library';
import { cleanupIndex, setPeriodicRescan } from '@/domains/library';

const store = useLibraryStore();
/** 当前连接是否 admin（远程浏览时隐藏库侧配置：只读 token 调管理端点只会 403） */
const isAdmin = computed(() => store.access === 'admin');

const current = ref('');
const isDefault = ref(true);
const error = ref('');
/** 迁移确认中的待选目录（确认对话框显示） */
const pending = ref<string | null>(null);
const busy = ref(false);
/** 迁移完成提示（从启动屏恢复回主界面后仍可见） */
const done = ref(false);

onMounted(() => {
  void shell.getCacheDir().then((d) => {
    current.value = d.current;
    isDefault.value = d.isDefault;
  });
});

async function pick() {
  const dir = await shell.pickCacheDir();
  if (dir) {
    error.value = '';
    pending.value = dir;
  }
}

async function confirmMigrate() {
  if (!pending.value) {
    return;
  }
  busy.value = true;
  error.value = '';
  const target = pending.value;
  pending.value = null;
  const failed = await shell.changeCacheDir(target);
  // 迁移期间主界面被切走（启动屏），恢复后本组件重新挂载——若未重新挂载则更新状态
  busy.value = false;
  if (failed) {
    error.value = failed;
  } else {
    current.value = target;
    isDefault.value = false;
    done.value = true;
  }
}

// ---- 周期兜底重扫（库级设置，写 .hawk/config.toml 的 [scan]）----
const periodicRescan = computed(() => store.library?.scan?.periodic ?? true);
async function onPeriodicRescan(e: Event) {
  await setPeriodicRescan((e.target as HTMLInputElement).checked);
}

/** 索引体检按钮状态（结果 toast 由 store 展示） */
const cleaning = ref(false);

async function runCleanup() {
  cleaning.value = true;
  try {
    await cleanupIndex();
  } finally {
    cleaning.value = false;
  }
}
</script>

<template>
  <div class="pane">
    <div v-if="isAdmin" class="switch-row">
      <div class="switch-label">周期兜底重扫</div>
      <label class="switch" title="周期兜底重扫">
        <input type="checkbox" :checked="periodicRescan" @change="onPeriodicRescan" />
        <span class="track" />
      </label>
    </div>

    <div class="field column">
      <span class="field-label">缓存目录</span>
      <span class="cache-path">{{ current || '…' }}</span>
    </div>

    <div v-if="isAdmin" class="field column">
      <span class="field-label">索引体检</span>
      <div class="actions-left">
        <button class="btn" :disabled="cleaning" @click="runCleanup">{{ cleaning ? '正在检查…' : '检查并清理缓存' }}</button>
      </div>
    </div>

    <div class="actions-left">
      <button class="btn" :disabled="busy" @click="pick">更改缓存目录…</button>
    </div>
    <p v-if="error" class="field-error">{{ error }}</p>
    <p v-if="done" class="hint">迁移完成，缓存已搬迁到新目录。</p>

    <!-- 迁移确认：整体搬迁 + 服务重启的事前说明 -->
    <div v-if="pending" class="migrate-confirm">
      <p>
        将缓存迁移到 <b>{{ pending }}</b
        >？现有缓存将整体搬迁（先复制后删除），期间后台服务会重启，素材库短暂不可用。
      </p>
      <div class="actions-left">
        <button class="btn danger" @click="confirmMigrate">开始迁移</button>
        <button class="btn" @click="pending = null">取消</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.cache-path {
  font-size: 13px;
  word-break: break-all;
  user-select: text;
}

.migrate-confirm {
  margin-top: 12px;
  padding: 12px;
  border: 1px solid var(--border);
  border-radius: 8px;
  font-size: 13px;
}

.migrate-confirm p {
  margin: 0 0 10px;
}
</style>

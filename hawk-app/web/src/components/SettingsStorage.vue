<script setup lang="ts">
// 存储分区（仅 Electron）：全局缓存父目录的查看与迁移、元数据存储方案切换。
// 缓存迁移为整体搬迁（先复制后删除），期间 server 重启（前端经 serverRestarting 切启动屏，
// 就绪后自动恢复——本对话框随主界面一同重现）。
// 存储方案切换：daemon 侧全量迁移（写新权威层+删旧文件），成功后经 shell.restartServer 重启生效。
import { computed, onMounted, ref } from 'vue';
import { api } from '../api/endpoints';
import { shell } from '../platform';
import { useLibraryStore } from '../stores/library';

const store = useLibraryStore();

// ---- 元数据存储方案 ----
/** 当前模式（library/info 的 storage_mode；数据库=默认本地，配置文件=网盘同步友好） */
const storageMode = computed(() => store.library?.storage_mode ?? 'database');
/** 待切换的目标模式（确认弹窗显示用；null=未在选择） */
const pendingMode = ref<'database' | 'toml' | null>(null);
const switching = ref(false);
const switchError = ref('');

/** 选择目标模式（点当前模式无动作） */
function switchTo(mode: 'database' | 'toml') {
  if (mode !== storageMode.value) {
    pendingMode.value = mode;
  }
}

/** 发起切换：daemon 全量迁移 → 主进程重启 server（启动屏接管，就绪后自动回主界面） */
async function confirmSwitch() {
  if (!pendingMode.value) {
    return;
  }
  switching.value = true;
  switchError.value = '';
  const target = pendingMode.value;
  try {
    await api.librarySetStorageMode(target);
    pendingMode.value = null;
    await shell.restartServer();
  } catch (e) {
    switchError.value = e instanceof Error ? e.message : String(e);
    switching.value = false;
  }
}

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
</script>

<template>
  <div class="pane">
    <div class="field column">
      <span class="field-label">元数据存储</span>
      <div class="mode-row">
        <label class="mode-option" :class="{ active: storageMode === 'database' }">
          <input type="radio" name="storage-mode" :checked="storageMode === 'database'" @change="switchTo('database')" />
          <span class="mode-name">数据库</span>
          <span class="hint">默认。单文件 .hawk/metadata.db，大批量操作快（事务写入）。适合不挂网盘同步的本地库。</span>
        </label>
        <label class="mode-option" :class="{ active: storageMode === 'toml' }">
          <input type="radio" name="storage-mode" :checked="storageMode === 'toml'" @change="switchTo('toml')" />
          <span class="mode-name">配置文件</span>
          <span class="hint">每素材一个 TOML 小文件，同步冲突粒度最小。库在 iCloud/Dropbox 等同步盘时必须选这个。</span>
        </label>
      </div>
      <p v-if="switchError" class="field-error">{{ switchError }}</p>
    </div>

    <div class="field column">
      <span class="field-label">缓存目录</span>
      <span class="cache-path">{{ current || '…' }}</span>
      <p class="hint">
        缩略图与索引缓存（派生物，可重建）统一存放在此目录下，各素材库按子目录区分。默认位于系统缓存目录；盘空间紧张时可迁移到其他盘。
      </p>
    </div>

    <div class="actions-left">
      <button class="btn" :disabled="busy" @click="pick">更改缓存目录…</button>
    </div>
    <p v-if="error" class="field-error">{{ error }}</p>
    <p v-if="done" class="hint">迁移完成，缓存已搬迁到新目录。</p>

    <!-- 迁移确认：整体搬迁 + 服务重启的事前说明 -->
    <div v-if="pending" class="migrate-confirm">
      <p>
        将缓存迁移到 <b>{{ pending }}</b>？现有缓存将整体搬迁（先复制后删除），期间后台服务会重启，素材库短暂不可用。
      </p>
      <div class="actions-left">
        <button class="btn danger" @click="confirmMigrate">开始迁移</button>
        <button class="btn" @click="pending = null">取消</button>
      </div>
    </div>

    <!-- 存储方案切换确认：全量迁移 + 自动重启 -->
    <div v-if="pendingMode" class="migrate-confirm">
      <p>
        切换为<b>{{ pendingMode === 'database' ? '数据库' : '配置文件' }}</b>方案？
        全部元数据将迁移到新存储（原数据迁移完成后自动删除），随后后台服务自动重启，素材库短暂不可用。
      </p>
      <div class="actions-left">
        <button class="btn danger" :disabled="switching" @click="confirmSwitch">{{ switching ? '正在迁移…' : '切换并重启' }}</button>
        <button class="btn" :disabled="switching" @click="pendingMode = null">取消</button>
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

.mode-row {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.mode-option {
  display: grid;
  grid-template-columns: auto auto 1fr;
  gap: 4px 8px;
  align-items: baseline;
  padding: 10px 12px;
  border: 1px solid var(--border);
  border-radius: 8px;
}

.mode-option.active {
  border-color: var(--accent);
}

.mode-option .hint {
  grid-column: 1 / -1;
}

.mode-name {
  color: var(--fg-0);
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

<script setup lang="ts">
// 存储分区（仅 Electron）：全局缓存父目录的查看与迁移。
// 迁移为整体搬迁（先复制后删除），期间 server 重启（前端经 serverRestarting 切启动屏，
// 就绪后自动恢复——本对话框随主界面一同重现）。
import { onMounted, ref } from 'vue';
import { shell } from '../platform';

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

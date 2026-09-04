<script setup lang="ts">
// 更新分区（仅 Electron）：通道切换 + 检查/下载/安装（状态机见 useUpdater）。
// 打开对话框即自载版本信息；v-show 保活（下载中切分区再回来进度不丢）。
import { computed, onMounted, ref } from 'vue';
import { shell } from '../platform';
import { useUpdater } from '../composables/useUpdater';

const updater = useUpdater();
const appVersion = ref('');
const buildSha = ref('');

/** 下载百分比（total 未知时为 null，UI 显示已下载字节数） */
const downloadPct = computed(() => {
  const p = updater.progress.value;
  return p && p.total > 0 ? Math.floor((p.received / p.total) * 100) : null;
});

function formatMB(bytes: number): string {
  return `${(bytes / (1 << 20)).toFixed(1)} MB`;
}

onMounted(() => {
  void shell.getAppVersion().then((v) => {
    appVersion.value = v.version;
    buildSha.value = v.sha;
  });
});
</script>

<template>
  <div class="pane">
    <div class="field">
      <span class="field-label">当前版本</span>
      <!-- 开发态显示「开发版」：package.json 保持下个发布版本，不追加 dev 后缀（避免发版前忘改回） -->
      <span class="update-current">{{
        !buildSha ? '…' : buildSha === 'dev' ? '开发版' : `v${appVersion} · ${buildSha.slice(0, 7)}`
      }}</span>
    </div>

    <div class="field column">
      <span class="field-label">更新通道</span>
      <div class="channel-row">
        <label class="radio">
          <input type="radio" value="stable" :checked="updater.channel.value === 'stable'" @change="updater.setChannel('stable')" />
          稳定版
        </label>
        <label class="radio">
          <input type="radio" value="nightly" :checked="updater.channel.value === 'nightly'" @change="updater.setChannel('nightly')" />
          滚动版
        </label>
      </div>
      <p class="hint">滚动包含最新改动，稳定性不作保证，开发人员专用</p>
    </div>

    <div class="field column">
      <span class="field-label">检查更新</span>
      <div class="update-status">
        <template v-if="updater.phase.value === 'checking'">正在检查…</template>
        <template v-else-if="updater.phase.value === 'uptodate'">已是最新（{{ updater.channel.value === 'nightly' ? 'nightly' : '稳定版' }}）</template>
        <template v-else-if="updater.update.value">
          发现新版本
          {{ updater.update.value.channel === 'nightly' ? `nightly ${updater.update.value.version}` : `v${updater.update.value.version}` }}
          <a :href="updater.update.value.url" target="_blank" rel="noreferrer">发布说明</a>
        </template>
        <template v-else>未检查</template>
        <p v-if="updater.error.value" class="field-error">{{ updater.error.value }}</p>
      </div>

      <!-- 下载进度 -->
      <div v-if="updater.phase.value === 'downloading'" class="update-progress">
        <div class="update-progress-bar">
          <div
            class="update-progress-fill"
            :style="{ width: downloadPct !== null ? `${downloadPct}%` : '100%' }"
            :class="{ indeterminate: downloadPct === null }"
          />
        </div>
        <span class="update-progress-text">
          {{
            updater.verifying.value
              ? '校验中…'
              : updater.progress.value
                ? `${formatMB(updater.progress.value.received)} / ${updater.progress.value.total > 0 ? formatMB(updater.progress.value.total) : '…'}`
                : '准备下载…'
          }}
        </span>
      </div>

      <div class="update-actions">
        <button
          :disabled="updater.phase.value === 'checking' || updater.phase.value === 'downloading' || updater.phase.value === 'ready'"
          @click="void updater.check()"
        >
          检查更新
        </button>
        <button
          v-if="updater.phase.value === 'available'"
          class="primary"
          @click="void updater.download()"
        >
          下载并安装
        </button>
        <button v-if="updater.phase.value === 'ready'" class="primary" @click="void updater.install()">
          重启并安装
        </button>
      </div>
      <p v-if="updater.phase.value === 'ready'" class="hint">重启后自动完成安装；已打开的局域网查看页在重启后刷新即可。</p>
    </div>
  </div>
</template>

<style src="./settings-shared.css"></style>

<style scoped>
.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

.update-current {
  color: var(--fg-1);
}

.channel-row {
  display: flex;
  gap: 16px;
}

.radio {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--fg-0);
}

.update-status {
  font-size: 13px;
  color: var(--fg-1);
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.update-status a {
  color: var(--accent);
  text-decoration: none;
}

.update-progress {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.update-progress-bar {
  height: 6px;
  border-radius: 3px;
  background: var(--bg-3);
  overflow: hidden;
}

.update-progress-fill {
  height: 100%;
  border-radius: 3px;
  background: var(--accent);
}

.update-progress-fill.indeterminate {
  width: 40%;
  animation: update-indeterminate 1.1s ease-in-out infinite;
}

@keyframes update-indeterminate {
  from {
    transform: translateX(-100%);
  }
  to {
    transform: translateX(250%);
  }
}

.update-progress-text {
  font-size: 12px;
  color: var(--fg-1);
  font-variant-numeric: tabular-nums;
}

.update-actions {
  display: flex;
  gap: 8px;
  margin-top: 4px;
}
</style>

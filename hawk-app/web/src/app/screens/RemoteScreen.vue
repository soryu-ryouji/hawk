<script setup lang="ts">
// 远程查看屏：远程查看模式（本地无库）的主屏。本地 daemon 以无库形态运行（仅远程管理面 +
// 会话代理），在此配置服务端/选择设备连接；连上远程库后连接描述符切换，boot 自动进主界面。
// 面板复用设置对话框的远程分区（同一 store 与 IPC 通道）；无库形态下隐藏分享向开关
// （writable / 允许其他设备访问——没有本地素材可服务）。
import { ref } from 'vue';
import { hasShell, shell } from '@/shared/lib/platform';
import { SettingsLanConnect, SettingsRemote } from '@/domains/settings';

/** 远程分区错误条（v-model:error 约定同设置对话框） */
const error = ref<string | null>(null);

/** 打开本地素材库：弹系统目录选择框，选定后主进程换库（启动屏接管，就绪进主界面） */
async function openLibrary() {
  if (hasShell) {
    await shell.selectLibrary();
  }
}
</script>

<template>
  <div class="remote-screen">
    <div class="head">
      <!-- 动态绑定避免 vite 按模块解析；icon.png 来自 publicDir（build/） -->
      <img class="logo" :src="'./icon.png'" alt="hawk" />
      <div class="title">连接素材库</div>
      <p class="hint">当前无本地素材库。可直接连接同一局域网内的 hawk-app，或经 hawk-server 远程浏览；素材不落本机。</p>
    </div>
    <div class="panel">
      <SettingsLanConnect />
      <h3 class="section-title">远程查看</h3>
      <SettingsRemote v-model:error="error" libraryless />
      <div v-if="error" class="error">{{ error }}</div>
    </div>
    <div class="foot">
      <button @click="openLibrary">打开本地素材库…</button>
    </div>
  </div>
</template>

<style scoped>
.remote-screen {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 16px;
  padding: 24px;
  overflow-y: auto;
}

.head {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  text-align: center;
}

.logo {
  width: 72px;
  height: 72px;
}

.title {
  font-size: 17px;
  font-weight: 600;
}

.hint {
  margin: 0;
  font-size: 12px;
  color: var(--text-tertiary, #999);
}

/* 面板复用设置分区样式（settings-shared.css 由 SettingsRemote 全局引入） */
.panel {
  width: min(520px, 100%);
  max-height: 60vh;
  overflow-y: auto;
  padding: 4px 16px;
  border: 1px solid var(--border, rgba(128, 128, 128, 0.3));
  border-radius: 10px;
}

.section-title {
  margin: 14px 0 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary, #aaa);
}

.error {
  padding: 8px 0 4px;
  color: var(--danger);
  font-size: 12px;
}

.foot {
  display: flex;
  gap: 10px;
}
</style>

<script setup lang="ts">
// 引导页：素材库未配置或失效时展示。打开本地库弹系统目录选择框；
// 「仅浏览远程素材库」拉起无库 daemon 进远程查看屏，不创建任何本地目录。
import { ref } from 'vue';
import { hasShell, shell } from '@/shared/lib/platform';

const emit = defineEmits<{ selected: [] }>();
const busy = ref(false);

async function openLibrary() {
  if (!hasShell) {
    return;
  }
  busy.value = true;
  try {
    // 选定后主进程拉起新 server；本页切启动屏，就绪经 server-started 事件进入主界面
    if (await shell.selectLibrary()) {
      emit('selected');
    }
  } finally {
    busy.value = false;
  }
}

/** 远程查看模式：主进程拉起无库 daemon，就绪后进远程查看屏（配置服务端/选设备） */
async function openRemote() {
  if (!hasShell) {
    return;
  }
  busy.value = true;
  try {
    if (await shell.openRemoteViewer()) {
      emit('selected');
    }
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="setup">
    <!-- 动态绑定避免 vite 按模块解析；icon.png 来自 publicDir（build/） -->
    <img class="logo" :src="'./icon.png'" alt="hawk" />
    <div class="options">
      <button class="open" :disabled="busy" @click="openLibrary">打开资源文件夹</button>
      <button class="open secondary" :disabled="busy" @click="openRemote">仅浏览远程素材库</button>
      <p class="hint">不在本机创建任何目录；接入服务端后即可浏览池内其他设备的素材库。</p>
    </div>
  </div>
</template>

<style scoped>
.setup {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 32px;
}

.logo {
  width: 128px;
  height: 128px;
}

.options {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
}

.open {
  padding: 10px 28px;
  font-size: 15px;
  border-color: var(--accent);
}

.open.secondary {
  border-color: var(--border);
}

.hint {
  margin: 0;
  font-size: 12px;
  color: var(--text-tertiary, #999);
  text-align: center;
}
</style>

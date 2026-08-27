<script setup lang="ts">
// 引导页：素材库未配置或失效时展示，点击按钮才弹系统目录选择框。
import { ref } from 'vue';

const busy = ref(false);

async function openLibrary() {
  if (!window.hawkShell) {
    return;
  }
  busy.value = true;
  try {
    // 选定后主进程会重启 server 并重载到主页；取消则留在本页
    await window.hawkShell.selectLibrary();
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="setup">
    <!-- 动态绑定避免 vite 按模块解析；icon.png 来自 publicDir（build/） -->
    <img class="logo" :src="'./icon.png'" alt="hawk" />
    <button class="open" :disabled="busy" @click="openLibrary">打开资源文件夹</button>
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

.open {
  padding: 10px 28px;
  font-size: 15px;
  border-color: var(--accent);
}
</style>

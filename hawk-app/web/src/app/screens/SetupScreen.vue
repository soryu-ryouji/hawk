<script setup lang="ts">
// 引导页：素材库未配置或失效时展示。打开本地库弹系统目录选择框；
// 没有本地库时也可以直接输入局域网地址 + 访问密钥浏览另一台 hawk。
import { ref } from 'vue';
import { hasShell, shell } from '@/shared/lib/platform';
import { ipcMessage } from '@/shared/connection';

const emit = defineEmits<{ selected: [] }>();
const busy = ref(false);
const lanOpen = ref(false);
const lanUrl = ref('');
const lanKey = ref('');
const lanError = ref('');

async function connectLan() {
  if (!hasShell) {
    return;
  }
  lanError.value = '';
  busy.value = true;
  try {
    await shell.lanConnectWithKey({ url: lanUrl.value, accessKey: lanKey.value });
    emit('selected');
  } catch (e) {
    lanError.value = ipcMessage(e);
  } finally {
    busy.value = false;
  }
}

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
</script>

<template>
  <div class="setup">
    <!-- 动态绑定避免 vite 按模块解析；icon.png 来自 publicDir（build/） -->
    <img class="logo" :src="'./icon.png'" alt="hawk" />
    <div class="options">
      <button class="open" :disabled="busy" @click="openLibrary">打开资源文件夹</button>
      <button v-if="hasShell" class="open secondary" :disabled="busy" @click="lanOpen = !lanOpen">连接局域网设备</button>
      <div v-if="hasShell && lanOpen" class="lan-form">
        <input v-model="lanUrl" type="text" placeholder="192.168.1.5:27375" autocomplete="off" spellcheck="false" />
        <input v-model="lanKey" type="password" placeholder="访问密钥 hk_..." autocomplete="off" spellcheck="false" />
        <button :disabled="busy || !lanUrl.trim() || !lanKey.trim()" @click="connectLan">连接</button>
        <p v-if="lanError" class="error">{{ lanError }}</p>
      </div>
      <p class="hint">hawk 不会把素材导入专有仓库；打开已有的素材文件夹即可开始。没有本地库时可直接连接局域网内已授权设备。</p>
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

.lan-form {
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: min(340px, calc(100vw - 40px));
}

.lan-form input {
  padding: 8px 10px;
}

.lan-form button {
  padding: 7px 14px;
  border-color: var(--accent);
}

.error {
  margin: 0;
  color: var(--danger);
  font-size: 12px;
  text-align: center;
}

.hint {
  margin: 0;
  font-size: 12px;
  color: var(--text-tertiary, #999);
  text-align: center;
}
</style>

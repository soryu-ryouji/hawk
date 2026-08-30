<script setup lang="ts">
// 局域网 web 查看的连接门页：浏览器直连 hawk-server 时无 Electron 注入的 token，
// 先输入访问 token，验证通过后经 client.storeToken 记住（localStorage 按 api host 隔离），
// 之后访问同一地址自动免输入直连。
import { ref } from 'vue';
import { apiConfig, setApiToken, storeToken, ApiError } from '../api/client';
import { api } from '../api/endpoints';

const emit = defineEmits<{ connect: [] }>();

const token = ref('');
const busy = ref(false);
const error = ref<string | null>(null);

async function submit() {
  const value = token.value.trim();
  if (!value || busy.value) {
    return;
  }
  busy.value = true;
  error.value = null;
  try {
    setApiToken(value);
    await api.appInfo(); // 验证 token（无效则 401 UNAUTHORIZED）
    storeToken(apiConfig().api, value);
    emit('connect');
  } catch (e) {
    if (e instanceof ApiError && e.code === 'UNAUTHORIZED') {
      error.value = 'token 无效，请确认后重试';
    } else {
      error.value = '无法连接素材库服务';
    }
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="connect">
    <img class="logo" :src="'./icon.png'" alt="hawk" />
    <div class="connect-card">
      <div class="title">连接素材库</div>
      <input
        v-model="token"
        type="password"
        placeholder="输入访问 token"
        autocomplete="off"
        @keydown.enter="submit"
      />
      <div v-if="error" class="error">{{ error }}</div>
      <button class="primary" :disabled="busy || !token.trim()" @click="submit">
        {{ busy ? '连接中…' : '连接' }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.connect {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 24px;
}

.logo {
  width: 96px;
  height: 96px;
}

.connect-card {
  width: 300px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 20px;
  border-radius: 8px;
  background: var(--bg-2);
  border: 1px solid var(--border);
}

.title {
  font-weight: 600;
  text-align: center;
}

.connect-card input {
  padding: 8px 10px;
}

.error {
  color: var(--danger);
  font-size: 12px;
  text-align: center;
}

.primary {
  padding: 8px 0;
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

.primary:disabled {
  opacity: 0.4;
  cursor: default;
}
</style>

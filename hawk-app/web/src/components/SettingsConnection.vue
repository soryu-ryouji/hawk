<script setup lang="ts">
// 连接分区（仅局域网 web 端）：当前访问级别 + token 注销（换身份重新输入）。
import { useLibraryStore } from '../stores/library';

const store = useLibraryStore();
const emit = defineEmits<{ logout: [] }>();
</script>

<template>
  <div class="pane">
    <div class="field">
      <span class="field-label">访问级别</span>
      <span>{{ store.viewerMode ? '只读' : '可读写' }}</span>
    </div>
    <p class="hint">本浏览器已记住当前 token；切换只读/可写身份时注销后重新输入另一个 token 即可。</p>
    <div class="actions-left">
      <button class="danger" @click="emit('logout')">注销 token</button>
    </div>
  </div>
</template>

<style src="./settings-shared.css"></style>

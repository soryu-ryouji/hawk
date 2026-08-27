<script setup lang="ts">
// 无边框窗口的最小化/最大化/关闭按钮（Windows 风格，固定右上）。
// 仅 Electron 内渲染；纯浏览器调试时 hawkShell 不存在，整体不显示。
import { ref } from 'vue';
import Icon from './Icon.vue';

const hasShell = !!window.hawkShell;
const isMaximized = ref(false);

// TitleBar/SetupScreen 均为顶层常驻组件，订阅随应用生命周期，不手动退订
window.hawkShell?.onWindowMaximized((maximized) => (isMaximized.value = maximized));

function minimize() {
  void window.hawkShell?.minimizeWindow();
}

async function toggleMaximize() {
  const maximized = await window.hawkShell?.toggleMaximizeWindow();
  if (typeof maximized === 'boolean') {
    isMaximized.value = maximized;
  }
}

function close() {
  void window.hawkShell?.closeWindow();
}
</script>

<template>
  <div v-if="hasShell" class="win-controls">
    <button class="win-btn" title="最小化" @click="minimize"><Icon name="winMinimize" :size="14" /></button>
    <button class="win-btn" :title="isMaximized ? '还原' : '最大化'" @click="toggleMaximize">
      <Icon :name="isMaximized ? 'winRestore' : 'winMaximize'" :size="12" />
    </button>
    <button class="win-btn close" title="关闭" @click="close"><Icon name="close" :size="14" /></button>
  </div>
</template>

<style scoped>
.win-controls {
  display: flex;
  align-self: stretch;
}

.win-btn {
  width: 42px;
  padding: 0;
  border: none;
  border-radius: 0;
  background: transparent;
  color: var(--fg-1);
  display: flex;
  align-items: center;
  justify-content: center;
}

.win-btn:hover {
  background: var(--bg-3);
  color: var(--fg-0);
}

.win-btn.close:hover {
  background: #e81123;
  color: #fff;
}
</style>

<script setup lang="ts">
// 无边框窗口的最小化/最大化/关闭按钮（Windows/Linux 风格，fixed 在窗口右上角）。
// macOS 用系统原生红绿灯（titleBarStyle: 'hidden'，压在侧栏顶部拖拽条上），本组件不渲染。
// 仅 Electron 内渲染；纯浏览器调试时 hawkShell 不存在，整体不显示。
import { ref } from 'vue';
import Icon from './Icon.vue';

const hasShell = !!window.hawkShell;
const isMac = window.hawkShell?.platform === 'darwin';
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
  <div v-if="hasShell && !isMac" class="win-controls">
    <button class="win-btn" title="最小化" @click="minimize"><Icon name="winMinimize" :size="14" /></button>
    <button class="win-btn" :title="isMaximized ? '还原' : '最大化'" @click="toggleMaximize">
      <Icon :name="isMaximized ? 'winRestore' : 'winMaximize'" :size="12" />
    </button>
    <button class="win-btn close" title="关闭" @click="close"><Icon name="close" :size="14" /></button>
  </div>
</template>

<style scoped>
.win-controls {
  /* fixed 而非放进某一栏：左右栏通高后右上角属检查器顶部拖拽条，且侧栏隐藏时位置不变；
     预览浮层（z-index 200）/对话框（150）仍盖得住它 */
  position: fixed;
  top: 0;
  right: 0;
  z-index: 100;
  display: flex;
  height: 40px;
  /* 退出窗口拖拽区：下方是检查器/标题栏的拖拽区域，缺了会被拖拽区拦截真实点击 */
  -webkit-app-region: no-drag;
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

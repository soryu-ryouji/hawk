import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';
import './styles.css';

// IME 组合态（中文输入法选词）中的 Enter/Escape 不下发给应用：
// Enter 是确认候选（不是提交）、Escape 是关闭候选窗（不是取消/关闭弹层）。
// 捕获阶段拦截，避免各输入框的 keydown 处理器误触发。
window.addEventListener(
  'keydown',
  (e) => {
    if ((e.isComposing || e.keyCode === 229) && (e.key === 'Enter' || e.key === 'Escape')) {
      e.stopPropagation();
    }
  },
  { capture: true },
);

createApp(App).use(createPinia()).mount('#app');

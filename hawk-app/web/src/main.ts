import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from '@/app/App.vue';
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

const app = createApp(App);
app.use(createPinia());

// 全局错误兜底：Vue 渲染/生命周期错误、未处理的 Promise 拒绝、同步异常。
// 统一 console 记录 + 轻提示（同一条消息 5s 内只提示一次，避免错误循环刷屏）；
// 业务错误仍在 store 内以 toast 呈现，这里只兜没人接的
const notified = new Map<string, number>();
function notifyError(scope: string, err: unknown): void {
  const detail = err instanceof Error ? `${err.name}: ${err.message}` : String(err);
  console.error(`[${scope}]`, err);
  const key = `${scope}:${detail}`;
  const now = Date.now();
  if (now - (notified.get(key) ?? 0) < 5000) {
    return;
  }
  notified.set(key, now);
  // 延迟取 store：错误可能发生在 pinia 安装完成前（此时只记录日志）
  import('@/domains/library').then(({ useLibraryStore }) => useLibraryStore().showToast(`界面错误（${scope}）：${detail.slice(0, 120)}`)).catch(() => {});
}

app.config.errorHandler = (err, _instance, info) => notifyError(info || 'vue', err);
window.addEventListener('unhandledrejection', (e) => notifyError('未处理的异步错误', e.reason));
window.addEventListener('error', (e) => notifyError('运行时错误', e.error ?? e.message));

app.mount('#app');

// 移动端判定：窄屏（竖屏手机）返回 true，驱动布局/交互切换（抽屉侧栏、点按开预览等）
import { onMounted, onUnmounted, ref } from 'vue';

const QUERY = '(max-width: 720px)';

export function useIsMobile() {
  const isMobile = ref(window.matchMedia(QUERY).matches);
  let mql = window.matchMedia(QUERY);
  const update = () => {
    isMobile.value = mql.matches;
    // 浮层经 Teleport 挂到 body（不在 .app 内），移动端样式需以 body.mobile 命中
    document.body.classList.toggle('mobile', mql.matches);
  };

  onMounted(() => {
    mql = window.matchMedia(QUERY);
    mql.addEventListener('change', update);
    update();
  });
  onUnmounted(() => mql.removeEventListener('change', update));

  return isMobile;
}

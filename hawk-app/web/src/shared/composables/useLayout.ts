// 布局模式：窄屏（手机、iPad 除 12.9" 横屏外全部、拖窄的桌面窗口）返回 narrow=true，宽屏桌面/平板为 wide。
// 断点取 1200px：三栏布局的最小健康宽度 = 220 侧栏 + ~700 顶栏固有最小内容 + 280 检查器，
// 低于此宽度中栏被顶栏最小内容撑破页面（grid 的 1fr = minmax(auto,1fr) 随内容最小宽度增长）。
// 触屏判定 touch：驱动触屏手势（下拉关闭预览、隐藏关闭 ×）；与 narrow 相互独立。
// 判定 = (pointer: coarse) 或 maxTouchPoints>0——iPad Safari 默认「请求桌面网站」时
// pointer: coarse 不命中（iPadOS 因支持触控板/鼠标上报精细指针），但 maxTouchPoints 恒大于 0；
// 漏判会把 iPad 当鼠标设备：点按变选择，而 iOS 双击不产生 dblclick，预览打不开。
// 设备/能力判断（Electron 能力、系统文案）走 platform.ts，不要引用本 composable。
import { onMounted, onUnmounted, ref } from 'vue';

const NARROW_QUERY = '(max-width: 1200px)';
const TOUCH_QUERY = '(pointer: coarse)';

/** 触屏能力综合判定（maxTouchPoints 运行时不变，无需监听其变化） */
function detectTouch(): boolean {
  return window.matchMedia(TOUCH_QUERY).matches || navigator.maxTouchPoints > 0;
}

export function useLayout() {
  const narrow = ref(window.matchMedia(NARROW_QUERY).matches);
  const touch = ref(detectTouch());
  let narrowMql = window.matchMedia(NARROW_QUERY);
  let touchMql = window.matchMedia(TOUCH_QUERY);
  const updateNarrow = () => {
    narrow.value = narrowMql.matches;
    // 浮层经 Teleport 挂到 body（不在 .app 内），窄屏样式需以 body.mobile 命中
    document.body.classList.toggle('mobile', narrowMql.matches);
  };
  const updateTouch = () => {
    touch.value = detectTouch();
    document.body.classList.toggle('touch', touch.value);
  };

  onMounted(() => {
    narrowMql = window.matchMedia(NARROW_QUERY);
    narrowMql.addEventListener('change', updateNarrow);
    updateNarrow();
    touchMql = window.matchMedia(TOUCH_QUERY);
    touchMql.addEventListener('change', updateTouch);
    updateTouch();
  });
  onUnmounted(() => {
    narrowMql.removeEventListener('change', updateNarrow);
    touchMql.removeEventListener('change', updateTouch);
  });

  return { narrow, touch };
}

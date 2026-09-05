// 长按检测（触屏/笔）：按住不动 500ms 触发，移动超阈值/抬手/取消即终止。
// iOS Safari 长按不派发 contextmenu（Android/桌面右键原生就有），本检测是移动端菜单的兜底；
// Android 上与原生 contextmenu 双触发时同一菜单被幂等重开（ContextMenu 的开场守卫挡掉秒关）。
export function useLongPress(onLongPress: (e: PointerEvent) => void) {
  const LONG_PRESS_MS = 500;
  /** 取消长按的位移半径（px）：网格滚动/滑动手势先行 */
  const MOVE_RADIUS = 10;

  let timer = 0;
  let origin: { x: number; y: number } | null = null;
  /** 本次按压是否已触发长按：click 兜吃用（触发后松手的 click 不再当单击） */
  let fired = false;

  function cancel(): void {
    if (timer) {
      clearTimeout(timer);
      timer = 0;
    }
    origin = null;
  }

  function down(e: PointerEvent): void {
    if (e.pointerType === 'mouse') {
      return; // 鼠标走右键，不参与长按
    }
    cancel();
    fired = false;
    origin = { x: e.clientX, y: e.clientY };
    timer = window.setTimeout(() => {
      timer = 0;
      fired = true;
      // 触觉反馈（Android；iOS 无 vibrate API，静默跳过）
      navigator.vibrate?.(10);
      onLongPress(e);
    }, LONG_PRESS_MS);
  }

  function move(e: PointerEvent): void {
    if (!origin) {
      return;
    }
    if (Math.hypot(e.clientX - origin.x, e.clientY - origin.y) > MOVE_RADIUS) {
      cancel();
    }
  }

  /** 长按触发后松手的首个 click 是否吞掉（吞掉返回 true，调用方跳过单击逻辑） */
  function consumeClick(): boolean {
    if (!fired) {
      return false;
    }
    fired = false;
    return true;
  }

  return { down, move, end: cancel, consumeClick };
}

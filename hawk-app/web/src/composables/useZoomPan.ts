// 预览浮层手势引擎：缩放/平移/滑动切换/下拉关闭的状态机（从 PreviewOverlay.vue 原样抽出）。
// 语义矩阵（抽取时的逐条核对规格，行为与原实现一致）：
// - 滚轮以光标为不动点缩放（MIN_SCALE–MAX_SCALE）
// - 双击：scale≤1 关闭预览；>1 复位（先还原再退出，不因双击误退）
// - 单指拖拽：scale>1 平移；=1 横向主导 → 滑动切换（过 56px 且有邻图才切），
//   触屏纵向向下主导 → 下拉关闭（向下 0.5 阻尼/向上 0.25 橡皮筋，过 96px 关闭）
// - 双指捏合：两指中点为不动点缩放，中点平移带动图片（捏合兼双指拖移）；收回到 ≤1 回到翻页模式
// - 双指变单指：放大态剩余手指无缝接管平移；捏合收尾不按滑动/下拉判定
// - 边缘橡皮筋：首/末张无邻图一侧拖动 0.35 阻尼（不可拖出空槽）
// - 点击 vs 拖拽：位移 >8px 记 moved，点击关闭/双击不再触发
// - 释放/关闭动画（170ms）期间不接收新手势
// 所有手势落在始终挂载的全屏手势层上（pointer capture），视觉层只是其下 pointer-events:none 的
// 跟随层——v-if 模式切换不打断进行中的手势（捏合跨 scale=1 不丢跟踪）。
import { ref, type Ref } from 'vue';

const MIN_SCALE = 0.1;
const MAX_SCALE = 20;
/** 触发切换的最小位移（CSS px） */
const SWIPE_MIN = 56;
/** 触发关闭的阻尼后位移 */
const PULL_CLOSE_MIN = 96;
/** 释放滑出/回弹过渡时长 */
const SWIPE_ANIM_MS = 170;

export interface ZoomPanOptions {
  /** 触屏布局（下拉关闭仅触屏可用；桌面纵向拖拽无语义） */
  touch: Ref<boolean>;
  /** 指定方向是否有邻图（边缘橡皮筋与可否切换） */
  hasNeighbor: (dir: 1 | -1) => boolean;
  navigate: (dir: 1 | -1) => void;
  close: () => void;
  /** 平移模式点击空白边距关闭的命中测试：点在实际图像显示区内返回 true（不关闭） */
  hitImage: (x: number, y: number) => boolean;
}

export function useZoomPan(opts: ZoomPanOptions) {
  const scale = ref(1);
  const tx = ref(0);
  const ty = ref(0);
  const dragging = ref(false);

  // 滑动切换（scale=1 时的横向手势）
  const swiping = ref(false); // 跟手阶段（意图已判定）
  const swipeAnim = ref(false); // 释放阶段（滑出/回弹过渡）
  const swipeX = ref(0);

  // 下拉关闭（移动端 scale=1 时的纵向手势；iOS 相册式：跟手+背景渐亮，过阈值松手滑出关闭）
  const pullActive = ref(false);
  const pullAnim = ref(false);
  const pullY = ref(0);

  // ---- 手势状态（非响应式：仅手势过程内使用）----
  /** 活动指针（pointerId → 最新位置）：双指捏合用 */
  const pointers = new Map<number, { x: number; y: number }>();
  let dragStart: { x: number; y: number; tx: number; ty: number } | null = null;
  /** 捏合进行中：起始指距与起始缩放 */
  let pinch: { startDist: number; startScale: number } | null = null;
  /** 本次按压是否已移动（区分点击与拖拽/捏合：移动过则点击关闭不触发） */
  let moved = false;

  /** 视觉层复位（切图/复位时调用；手指通常已抬起，手势状态一并清零兜底防泄漏） */
  function reset() {
    scale.value = 1;
    tx.value = 0;
    ty.value = 0;
    swiping.value = false;
    swipeAnim.value = false;
    swipeX.value = 0;
    pullActive.value = false;
    pullAnim.value = false;
    pullY.value = 0;
    pointers.clear();
    pinch = null;
    dragStart = null;
    dragging.value = false;
    moved = false;
  }

  /** 双击：未放大（scale≤1）时退出预览（与双击卡片开预览对称）；放大状态仍复位
   * （放大看细节后先还原再退出，不因双击误退） */
  function onDblClick() {
    if (scale.value <= 1) {
      opts.close();
      return;
    }
    reset();
  }

  function onWheel(e: WheelEvent) {
    const next = Math.min(Math.max(scale.value * Math.exp(-e.deltaY * 0.002), MIN_SCALE), MAX_SCALE);
    if (next === scale.value) {
      return;
    }
    // 光标（相对视口中心）处的图像点保持不动：t' = c - k(c - t)
    const cx = e.clientX - window.innerWidth / 2;
    const cy = e.clientY - window.innerHeight / 2;
    const k = next / scale.value;
    tx.value = cx - (cx - tx.value) * k;
    ty.value = cy - (cy - ty.value) * k;
    scale.value = next;
  }

  function onPointerDown(e: PointerEvent) {
    if (e.pointerType === 'mouse' && e.button !== 0) {
      return;
    }
    if (swipeAnim.value || pullAnim.value) {
      return; // 释放动画期间不接收新手势
    }
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    moved = false;
    if (pointers.size === 2) {
      // 第二指落下：转捏合，取消进行中的单指手势（平移/滑动/下拉）
      const [a, b] = [...pointers.values()];
      pinch = { startDist: Math.hypot(a.x - b.x, a.y - b.y), startScale: scale.value };
      dragStart = null;
      dragging.value = false;
      swiping.value = false;
      swipeX.value = 0;
      pullActive.value = false;
      pullY.value = 0;
    } else if (pointers.size === 1) {
      dragStart = { x: e.clientX, y: e.clientY, tx: tx.value, ty: ty.value };
      dragging.value = true;
    }
  }

  function onPointerMove(e: PointerEvent) {
    if (!pointers.has(e.pointerId)) {
      return;
    }
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pinch) {
      if (pointers.size < 2) {
        return;
      }
      const [a, b] = [...pointers.values()];
      const dist = Math.hypot(a.x - b.x, a.y - b.y);
      const next = Math.min(Math.max(pinch.startScale * (dist / pinch.startDist), MIN_SCALE), MAX_SCALE);
      // 两指中点为不动点缩放；中点本身的平移带动图片（捏合兼双指拖移）
      const cx = (a.x + b.x) / 2 - window.innerWidth / 2;
      const cy = (a.y + b.y) / 2 - window.innerHeight / 2;
      const k = next / scale.value;
      tx.value = cx - (cx - tx.value) * k;
      ty.value = cy - (cy - ty.value) * k;
      scale.value = next;
      moved = true;
      return;
    }
    if (!dragStart) {
      return;
    }
    const dx = e.clientX - dragStart.x;
    const dy = e.clientY - dragStart.y;
    if (Math.abs(dx) > 8 || Math.abs(dy) > 8) {
      moved = true;
    }
    if (scale.value > 1) {
      tx.value = dragStart.tx + dx;
      ty.value = dragStart.ty + dy;
      return;
    }
    // 缩放=1：横向主导 → 滑动切换意图；纵向向下主导（仅触屏）→ 下拉关闭意图
    if (!swiping.value && !pullActive.value) {
      if (Math.abs(dx) > 8 && Math.abs(dx) > Math.abs(dy)) {
        swiping.value = true;
      } else if (opts.touch.value && dy > 8 && dy > Math.abs(dx)) {
        pullActive.value = true;
      }
    }
    if (pullActive.value) {
      // 向下 0.5 阻尼跟手，向上轻微跟手（rubber-band 质感）
      pullY.value = dy > 0 ? dy * 0.5 : dy * 0.25;
      return;
    }
    if (swiping.value) {
      // 边缘橡皮筋：首/末张无邻图一侧,拖动受阻尼（不可拖出空槽）
      const hasTarget = dx < 0 ? opts.hasNeighbor(1) : opts.hasNeighbor(-1);
      swipeX.value = hasTarget ? dx : dx * 0.35;
    }
  }

  function onPointerUp(e: PointerEvent) {
    if (!pointers.has(e.pointerId)) {
      return;
    }
    pointers.delete(e.pointerId);
    if (pinch) {
      if (pointers.size >= 2) {
        return; // 仍有两指：捏合继续
      }
      pinch = null;
      // 双指变单指：剩余手指无缝接管平移（捏合收尾不按滑动/下拉判定）
      if (pointers.size === 1 && scale.value > 1) {
        const [p] = pointers.values();
        dragStart = { x: p.x, y: p.y, tx: tx.value, ty: ty.value };
        dragging.value = true;
      }
      return;
    }
    if (!dragStart) {
      return;
    }
    dragStart = null;
    dragging.value = false;
    // 下拉释放：过阈值 → 下滑出 + 背景淡出后关闭；否则回弹（仅移动端会进入 pullActive）
    if (pullActive.value) {
      const shouldClose = pullY.value >= PULL_CLOSE_MIN;
      pullAnim.value = true;
      if (shouldClose) {
        pullY.value = window.innerHeight;
        setTimeout(() => opts.close(), SWIPE_ANIM_MS);
      } else {
        pullY.value = 0;
        setTimeout(() => {
          pullActive.value = false;
          pullAnim.value = false;
        }, SWIPE_ANIM_MS);
      }
      return;
    }
    if (!swiping.value) {
      return;
    }
    // 释放：过阈值且有目标 → 轨道继续滑动使邻图落位中央,动画结束提交切换并无缝复位;否则回弹
    const dir: 1 | -1 = swipeX.value < 0 ? 1 : -1;
    const canNavigate = Math.abs(swipeX.value) >= SWIPE_MIN && (dir === 1 ? opts.hasNeighbor(1) : opts.hasNeighbor(-1));
    swipeAnim.value = true;
    if (canNavigate) {
      swipeX.value = -dir * window.innerWidth;
      setTimeout(() => {
        opts.navigate(dir);
        // 提交后轨道静默复位:新邻图已是中央帧,视觉无缝(watch 随即再清一次,同值无害)
        swipeAnim.value = false;
        swipeX.value = 0;
      }, SWIPE_ANIM_MS);
    } else {
      swipeX.value = 0;
      setTimeout(() => {
        swiping.value = false;
        swipeAnim.value = false;
      }, SWIPE_ANIM_MS);
    }
  }

  function onPointerCancel(e: PointerEvent) {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) {
      pinch = null;
    }
    dragStart = null;
    dragging.value = false;
    swiping.value = false;
    swipeX.value = 0;
    pullActive.value = false;
    pullY.value = 0;
  }

  /**
   * 平移模式（缩放>1）点击空白边距关闭；点在图像上、有拖动、或缩放=1（carousel）时不响应——
   * carousel 点击不关闭，平移模式点图像不关闭。
   */
  function onGestureClick(e: MouseEvent) {
    if (moved || scale.value <= 1 || opts.hitImage(e.clientX, e.clientY)) {
      return;
    }
    opts.close();
  }

  return {
    scale, tx, ty, dragging,
    swiping, swipeAnim, swipeX,
    pullActive, pullAnim, pullY,
    reset,
    onWheel, onPointerDown, onPointerMove, onPointerUp, onPointerCancel, onDblClick, onGestureClick,
  };
}

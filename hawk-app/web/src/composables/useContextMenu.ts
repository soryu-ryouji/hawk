// 全局唯一右键菜单：模块级单例状态，ContextMenu.vue 渲染。
import { reactive, readonly } from 'vue';
import type { MenuItem } from '../types';

const state = reactive({
  visible: false,
  x: 0,
  y: 0,
  /** 按钮锚点（getBoundingClientRect 快照）：提供时菜单从锚点下方对齐展开（下拉模式），
   *  避免从鼠标/手指点展开遮挡触发按钮本身；缺省为鼠标位置（右键语义） */
  anchor: null as { left: number; right: number; top: number; bottom: number } | null,
  items: [] as MenuItem[],
  /** 最近一次打开的时刻：开场守卫用（Android 长按原生 contextmenu 与长按检测双触发、
   *  长按后松手跟发的 click 落在遮罩上时，防止刚开的菜单被秒关） */
  openedAt: 0,
});

/** 开场守卫窗口：人手不可能在开菜单后这么快点到遮罩，只过滤程序性跟发事件。
 *  消费方：ContextMenu.vue 的遮罩 click/contextmenu 判定 */
export const CONTEXT_MENU_OPEN_GUARD_MS = 250;

export function useContextMenu() {
  function open(items: MenuItem[], e: MouseEvent, anchorEl?: HTMLElement) {
    state.items = items;
    state.anchor = anchorEl ? anchorEl.getBoundingClientRect() : null;
    state.x = e.clientX;
    state.y = e.clientY;
    state.visible = true;
    state.openedAt = Date.now();
  }

  function close() {
    state.visible = false;
  }

  /** 原位替换菜单项（项内操作引起列表变化时用，如删除历史素材库记录后刷新，菜单不跳位） */
  function setItems(items: MenuItem[]) {
    state.items = items;
  }

  return { state: readonly(state), open, close, setItems };
}

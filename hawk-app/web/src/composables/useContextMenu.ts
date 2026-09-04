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
});

export function useContextMenu() {
  function open(items: MenuItem[], e: MouseEvent, anchorEl?: HTMLElement) {
    state.items = items;
    state.anchor = anchorEl ? anchorEl.getBoundingClientRect() : null;
    state.x = e.clientX;
    state.y = e.clientY;
    state.visible = true;
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

// 全局唯一右键菜单：模块级单例状态，ContextMenu.vue 渲染。
import { reactive, readonly } from 'vue';
import type { MenuItem } from '../types';

const state = reactive({
  visible: false,
  x: 0,
  y: 0,
  items: [] as MenuItem[],
});

export function useContextMenu() {
  function open(items: MenuItem[], e: MouseEvent) {
    state.items = items;
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

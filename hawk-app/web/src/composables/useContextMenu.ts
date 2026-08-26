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

  return { state: readonly(state), open, close };
}

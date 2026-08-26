// 全局快捷键：焦点在输入框时跳过；Delete 回收/恢复、Esc 关浮层、Cmd/Ctrl+A 全选、←/→ 切换预览。
import { useEventListener } from '@vueuse/core';
import { useLibraryStore } from '../stores/library';
import { useContextMenu } from './useContextMenu';

export function useShortcuts() {
  const store = useLibraryStore();
  const menu = useContextMenu();

  useEventListener(window, 'keydown', (e: KeyboardEvent) => {
    const target = e.target as HTMLElement | null;
    if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) {
      return;
    }

    if (e.key === 'Escape') {
      if (store.previewId) {
        store.closePreview();
      } else {
        menu.close();
      }
      return;
    }

    if (e.key === 'Delete' || e.key === 'Backspace') {
      if (store.selection.length > 0) {
        void (store.isTrash ? store.restoreSelected() : store.trashSelected());
      }
      return;
    }

    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      store.selectAll();
      return;
    }

    if (store.previewId && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
      store.navigatePreview(e.key === 'ArrowRight' ? 1 : -1);
    }
  });
}

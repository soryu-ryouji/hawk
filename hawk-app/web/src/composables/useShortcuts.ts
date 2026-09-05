// 全局快捷键：焦点在输入框时跳过。
// 空格 展开/关闭预览；←→ 预览中切换图片；方向键 网格中移动选中框；
// Delete 回收/恢复、Esc 关浮层、Cmd/Ctrl+A 全选。
import { useEventListener } from '@vueuse/core';
import { useLibraryStore } from '../stores/library';
import { usePreviewStore } from '../stores/preview';
import { useContextMenu } from './useContextMenu';
import { gridNavRows, markKeyboardNavScroll, moveGridSelection } from './useGridNav';
import { itemKey } from '../viewLogic';

export function useShortcuts() {
  const store = useLibraryStore();
  const preview = usePreviewStore();
  const menu = useContextMenu();

  useEventListener(window, 'keydown', (e: KeyboardEvent) => {
    const target = e.target as HTMLElement | null;
    if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) {
      return;
    }

    // 图片编辑窗口打开时接管全部按键(窗口自带 Esc/关闭逻辑),全局快捷键一律让行——
    // 否则 Esc 会关掉底层预览、Delete 会删掉正在编辑的素材
    if (preview.editorTarget) {
      return;
    }

    if (e.key === 'Escape') {
      if (preview.previewId) {
        preview.closePreview();
      } else {
        menu.close();
      }
      return;
    }

    if (e.key === ' ') {
      e.preventDefault(); // 阻止页面滚动
      if (preview.previewId) {
        preview.closePreview();
      } else if (store.primarySelected) {
        preview.openPreview(itemKey(store.primarySelected.id, store.primarySelected.path));
      }
      return;
    }

    if (e.key === 'Delete' || e.key === 'Backspace') {
      // 只读查看（局域网 viewer）：写操作快捷键禁用
      if (store.viewerMode) {
        return;
      }
      if (store.selection.length > 0) {
        void (store.isTrash ? store.restoreSelected() : store.trashSelected());
      }
      return;
    }

    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      if (!e.repeat) {
        // 按住不放时 keydown 自动重复：全选幂等，重复触发只是白重建选择集
        store.selectAll();
      }
      return;
    }

    if (preview.previewId) {
      // 预览中：←→ 切换图片，其余方向键不落到网格
      if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
        e.preventDefault();
        preview.navigatePreview(e.key === 'ArrowRight' ? 1 : -1);
      }
      return;
    }

    if (e.key.startsWith('Arrow')) {
      e.preventDefault();
      const dx = e.key === 'ArrowLeft' ? -1 : e.key === 'ArrowRight' ? 1 : 0;
      const dy = e.key === 'ArrowUp' ? -1 : e.key === 'ArrowDown' ? 1 : 0;
      const next = moveGridSelection(
        gridNavRows.value,
        store.primarySelected ? itemKey(store.primarySelected.id, store.primarySelected.path) : null,
        dx,
        dy,
      );
      if (next) {
        // 仅键盘导航允许触发「滚动到选中项」（ItemGrid 的 watcher 消费此标记）
        markKeyboardNavScroll();
        store.select(next);
      }
    }
  });
}

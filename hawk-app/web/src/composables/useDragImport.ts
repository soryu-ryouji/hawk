// 拖拽导入：drop 到窗口任意处；文件夹经 webkitGetAsEntry 递归展开（收集逻辑在 importer store）。
// Electron 经 preload 的 webUtils.getPathForFile 取绝对路径逐个入库；
// 浏览器（局域网 web 端）无路径可取，改为读 File 内容逐个 multipart 上传。
// 侧栏（文件夹树）的结构化导入在 Sidebar/FolderTreeNode 的 drop 处理中调 importer.importEntries，
// 与本文件无关。
import { useDropZone } from '@vueuse/core';
import { useLibraryStore } from '@/domains/library';
import { useImporterStore } from '@/stores/importer';
import { ITEMS_MIME, droppedEntries } from '../dnd';

export function useDragImport() {
  const store = useLibraryStore();
  const importer = useImporterStore();

  useDropZone(document, {
    onDrop: async (files, event) => {
      // 库内素材拖拽（拖到侧栏文件夹/分类/标签）：不是文件导入，静默忽略
      if (event.dataTransfer?.types.includes(ITEMS_MIME)) {
        return;
      }
      // 只读查看（局域网 viewer）的拒绝提示在 importer.importEntries 内统一给出
      // （悬停期间本 composable 的 onOver 已把光标置为禁止）
      const entries = droppedEntries(event);
      await importer.importEntries(entries, undefined, files);
    },
    // 仅库内视图可导入；只读查看与库内素材拖拽（仅侧栏是合法放置区）显式禁止，
    // 避免 document 级处理器给出误导性的 copy 光标
    onOver: (_files, event) => {
      const dt = event.dataTransfer;
      if (!dt) {
        return;
      }
      if (store.viewerMode || store.isTrash || (dt.types.includes(ITEMS_MIME) && !(event.target as HTMLElement).closest('.sidebar'))) {
        dt.dropEffect = 'none';
      }
    },
  });
}

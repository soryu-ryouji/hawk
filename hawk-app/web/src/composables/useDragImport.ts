// 拖拽导入：drop 到窗口任意处；文件夹经 webkitGetAsEntry 递归展开为文件，
// 再经 preload 的 webUtils.getPathForFile 取绝对路径逐个入库。
import { useDropZone } from '@vueuse/core';
import { useLibraryStore } from '../stores/library';
import { ITEMS_MIME } from '../dnd';

async function* walkEntry(entry: FileSystemEntry): AsyncGenerator<File> {
  if (entry.isFile) {
    yield await new Promise<File>((resolve, reject) => (entry as FileSystemFileEntry).file(resolve, reject));
    return;
  }
  if (entry.isDirectory) {
    const reader = (entry as FileSystemDirectoryEntry).createReader();
    // readEntries 每批最多 100 条，必须循环读到空
    for (;;) {
      const batch = await new Promise<FileSystemEntry[]>((resolve, reject) => reader.readEntries(resolve, reject));
      if (batch.length === 0) {
        break;
      }
      for (const child of batch) {
        yield* walkEntry(child);
      }
    }
  }
}

export function useDragImport() {
  const store = useLibraryStore();

  useDropZone(document, {
    onDrop: async (_files, event) => {
      // 库内素材拖拽（拖到侧栏文件夹/分类/标签）：不是文件导入，静默忽略
      if (event.dataTransfer?.types.includes(ITEMS_MIME)) {
        return;
      }
      const shell = window.hawkShell;
      if (!shell) {
        store.showToast('浏览器模式不支持导入（无法取文件路径）');
        return;
      }
      // 落下即占用导入态：文件夹递归收集可能耗时，期间进度条显示「正在收集文件」
      if (!store.importBegin()) {
        return;
      }

      const entries = [...(event.dataTransfer?.items ?? [])]
        .map((item) => item.webkitGetAsEntry())
        .filter((entry): entry is FileSystemEntry => entry !== null);

      const paths: string[] = [];
      try {
        for (const entry of entries) {
          for await (const file of walkEntry(entry)) {
            const abs = shell.getPathForFile(file);
            if (abs) {
              paths.push(abs);
            }
          }
        }
      } catch {
        store.showToast('读取文件列表失败');
      }
      await store.importPaths(paths);
    },
    // 仅库内视图可导入；库内素材拖拽只有侧栏是合法放置区（行级处理器已置 move），
    // 悬停在网格等其他位置时显式禁止，避免 document 级处理器给出误导性的 copy 光标
    onOver: (_files, event) => {
      const dt = event.dataTransfer;
      if (!dt) {
        return;
      }
      if (store.isTrash || (dt.types.includes(ITEMS_MIME) && !(event.target as HTMLElement).closest('.sidebar'))) {
        dt.dropEffect = 'none';
      }
    },
  });
}

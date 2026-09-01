// 拖拽导入：drop 到窗口任意处；文件夹经 webkitGetAsEntry 递归展开为文件。
// Electron 经 preload 的 webUtils.getPathForFile 取绝对路径逐个入库；
// 浏览器（局域网 web 端）无路径可取，改为读 File 内容逐个 multipart 上传。
import { useDropZone } from '@vueuse/core';
import { useLibraryStore } from '../stores/library';
import { useImporterStore } from '../stores/importer';
import { hasShell, shell } from '../platform';
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
  const importer = useImporterStore();

  useDropZone(document, {
    onDrop: async (_files, event) => {
      // 库内素材拖拽（拖到侧栏文件夹/分类/标签）：不是文件导入，静默忽略
      if (event.dataTransfer?.types.includes(ITEMS_MIME)) {
        return;
      }
      // 只读查看（局域网 viewer）：导入即写操作，给出明确反馈（悬停光标也已置为禁止）
      if (store.viewerMode) {
        store.showToast('只读模式无法导入，需使用可写 token');
        return;
      }
      // 落下即占用导入态：文件夹递归收集可能耗时，期间进度条显示「正在收集文件」
      if (!importer.importBegin()) {
        return;
      }
      const entries = [...(event.dataTransfer?.items ?? [])]
        .map((item) => item.webkitGetAsEntry())
        .filter((entry): entry is FileSystemEntry => entry !== null);

      try {
        if (hasShell) {
          const paths: string[] = [];
          for (const entry of entries) {
            for await (const file of walkEntry(entry)) {
              const abs = shell.getPathForFile(file);
              if (abs) {
                paths.push(abs);
              }
            }
          }
          await importer.importPaths(paths);
        } else {
          const files: File[] = [];
          for (const entry of entries) {
            for await (const file of walkEntry(entry)) {
              files.push(file);
            }
          }
          // webkitGetAsEntry 不可用/返回空的浏览器：退回平铺文件列表（无文件夹递归展开）
          if (files.length === 0 && _files?.length) {
            files.push(..._files);
          }
          await importer.importFiles(files);
        }
      } catch {
        store.showToast('读取文件列表失败');
      }
    },
    // 仅库内视图可导入；只读查看与库内素材拖拽（仅侧栏是合法放置区）显式禁止，
    // 避免 document 级处理器给出误导性的 copy 光标
    onOver: (_files, event) => {
      const dt = event.dataTransfer;
      if (!dt) {
        return;
      }
      if (
        store.viewerMode ||
        store.isTrash ||
        (dt.types.includes(ITEMS_MIME) && !(event.target as HTMLElement).closest('.sidebar'))
      ) {
        dt.dropEffect = 'none';
      }
    },
  });
}

// 导入域子 store：批量导入的进度指示、重复策略弹窗与两个入口（路径/文件）。
// 批量循环状态机在 importBatch.runImportBatch（依赖注入，可单测），这里只做 store 接线。
// 引用规则：可读主 store 的 state/getter、调其 action；主 store 不反向依赖本 store。
import { ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '../api/endpoints';
import { runImportBatch } from '../importBatch';
import { useLibraryStore } from './library';

export const useImporterStore = defineStore('importer', () => {
  const library = useLibraryStore();

  /** 导入进度：null 无任务；total=0 表示收集文件阶段（不定态），done 为已处理数 */
  const importProgress = ref<{ total: number; done: number } | null>(null);

  // ---- 导入重复策略：首个「内容已在库内」时暂停并问一次（ImportDuplicateDialog 呈现，App.vue 挂载），
  // 选择对整批生效——逐文件弹窗在批量导入下不可用 ----
  const dupPrompt = ref<null | ((choice: 'skip' | 'import') => void)>(null);

  function askDuplicatePolicy(): Promise<'skip' | 'import'> {
    return new Promise((resolve) => {
      dupPrompt.value = resolve;
    });
  }

  function resolveDuplicatePolicy(choice: 'skip' | 'import') {
    dupPrompt.value?.(choice);
    dupPrompt.value = null;
  }

  /** 导入开始：拖拽落下即调用，覆盖「收集文件」阶段；已有任务时拒绝并提示 */
  function importBegin(): boolean {
    if (importProgress.value) {
      library.showToast('已有导入任务进行中');
      return false;
    }
    importProgress.value = { total: 0, done: 0 };
    return true;
  }

  /** 拖拽导入：逐个 itemAddByPath（server 逐文件完成复制/哈希/索引/缩略图后才返回），done 逐项推进 */
  async function importPaths(paths: string[]) {
    await runImportBatch(paths, {
      importOne: (path, skipExisting) =>
        api.itemAddByPath(path, {
          folder_path: library.currentFolderPath ?? undefined,
          skip_existing: skipExisting,
        }),
      askPolicy: askDuplicatePolicy,
      setProgress: (p) => (importProgress.value = p),
      onEmpty: () => library.showToast('未找到可导入的文件'),
      onSummary: (c) =>
        library.showToast(
          `导入完成：新增 ${c.added}${c.skipped ? `，忽略重复 ${c.skipped}` : ''}${c.existed ? `，重复导入 ${c.existed}` : ''}${c.failed ? `，失败 ${c.failed}` : ''}`,
        ),
    });
    // SSE item.added 已触发防抖骨架重载，这里不重复拉取
  }

  /** 浏览器端导入（无 hawkShell，拖拽/文件选择器拿到的是 File 内容）：逐个 multipart 上传。
   * 重复策略与 importPaths 一致（首问后整批生效） */
  async function importFiles(files: File[]) {
    await runImportBatch(files, {
      importOne: (file, skipExisting) =>
        api.itemUpload(file, {
          folder_path: library.currentFolderPath ?? undefined,
          skip_existing: skipExisting,
        }),
      askPolicy: askDuplicatePolicy,
      setProgress: (p) => (importProgress.value = p),
      onEmpty: () => library.showToast('未找到可导入的文件'),
      onSummary: (c) =>
        library.showToast(
          `上传完成：新增 ${c.added}${c.skipped ? `，忽略重复 ${c.skipped}` : ''}${c.existed ? `，重复导入 ${c.existed}` : ''}${c.failed ? `，失败 ${c.failed}` : ''}`,
        ),
    });
  }

  return { importProgress, dupPrompt, resolveDuplicatePolicy, importBegin, importPaths, importFiles };
});

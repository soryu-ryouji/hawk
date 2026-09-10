// 导入域子 store：批量导入的进度指示、重复策略弹窗与拖拽入口（平铺/结构化）。
// 批量循环状态机在 importBatch.runImportBatch（依赖注入，可单测），这里只做 store 接线。
// 引用规则：可读主 store 的 state/getter、调其 action；主 store 不反向依赖本 store。
import { ref } from 'vue';
import { defineStore } from 'pinia';
import { api } from '@/shared/api/endpoints';
import { runImportBatch } from '../importBatch';
import { hasShell, shell } from '@/shared/lib/platform';
import { useLibraryStore } from './library';

/** 拖入展开后的一个文件：relPath 相对拖入根（含拖入目录自身的名字） */
interface DroppedFile {
  file: File;
  relPath: string;
}

/** 递归展开 entry：文件收集进 files（带相对路径），途经目录（含空目录）记入 dirs（父先子后）；
 *  '.' 开头的隐藏条目跳过（与服务端侧栏树的 is_hidden 过滤同口径，建了也不可见） */
async function collectEntry(entry: FileSystemEntry, prefix: string, files: DroppedFile[], dirs: Set<string>): Promise<void> {
  if (entry.isFile) {
    if (!entry.name.startsWith('.')) {
      files.push({
        file: await new Promise<File>((resolve, reject) => (entry as FileSystemFileEntry).file(resolve, reject)),
        relPath: prefix + entry.name,
      });
    }
    return;
  }
  if (entry.isDirectory) {
    if (entry.name.startsWith('.')) {
      return;
    }
    const dir = prefix + entry.name;
    dirs.add(dir);
    const reader = (entry as FileSystemDirectoryEntry).createReader();
    // readEntries 每批最多 100 条，必须循环读到空
    for (;;) {
      const batch = await new Promise<FileSystemEntry[]>((resolve, reject) => reader.readEntries(resolve, reject));
      if (batch.length === 0) {
        break;
      }
      for (const child of batch) {
        await collectEntry(child, `${dir}/`, files, dirs);
      }
    }
  }
}

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

  /** 拖拽导入统一入口：document 级平铺（targetFolder 缺省 → 当前视图文件夹，网格 drop 的行为）
   *  与侧栏结构化（targetFolder 指定 → 在目标下重建拖入目录树，文件按映射路径入库）。
   *  Electron 逐文件 item/add（路径导入，保留时间戳），浏览器逐文件 multipart upload；
   *  重复策略首问后整批生效。fallbackFiles：entries 不可用的拖拽源退回的平铺文件列表
   *  （无目录信息，直落目标文件夹） */
  async function importEntries(entries: FileSystemEntry[], targetFolder?: string, fallbackFiles?: File[] | null) {
    if (library.viewerMode) {
      library.showToast('只读模式无法导入，需使用可写 token');
      return;
    }
    // 落下即占用导入态：文件夹递归收集可能耗时，期间进度条显示「正在收集文件」
    if (!importBegin()) {
      return;
    }
    try {
      const files: DroppedFile[] = [];
      const dirs = new Set<string>();
      for (const entry of entries) {
        await collectEntry(entry, '', files, dirs);
      }
      // entries 不可用/返回空的拖拽源：退回平铺文件列表（无文件夹递归展开）。
      // 结构化模式下拖入的是空文件夹（dirs 非空）不退回——那正是要添加的目录
      if (files.length === 0 && dirs.size === 0 && fallbackFiles?.length) {
        files.push(...fallbackFiles.filter((file) => !file.name.startsWith('.')).map((file) => ({ file, relPath: file.name })));
      }
      // Electron 下拿不到绝对路径的条目（异常情形）剔除，避免逐项报「文件不存在」
      if (hasShell) {
        for (let i = files.length - 1; i >= 0; i--) {
          if (!shell.getPathForFile(files[i].file)) {
            files.splice(i, 1);
          }
        }
      }

      if (targetFolder !== undefined) {
        // 结构化：先建目录树（含空目录，立即出现在侧栏；同名目录已在则静默合并导入），再逐文件按映射入库
        await ensureFolders(dirs, targetFolder);
        if (files.length === 0) {
          importProgress.value = null;
          library.showToast(dirs.size > 0 ? `已添加 ${dirs.size} 个空文件夹` : '未找到可导入的文件');
          return;
        }
        await runBatch(files, (relPath) => joinFolder(targetFolder, dirOf(relPath)), '导入');
      } else {
        // 平铺：目录结构拍扁，全部落当前视图文件夹
        const current = library.currentFolderPath ?? '';
        await runBatch(files, () => current, '导入');
      }
    } catch {
      importProgress.value = null;
      library.showToast('读取文件列表失败');
    }
  }

  /** 浏览器端导入（无 hawkShell，拖拽/文件选择器拿到的是 File 内容）：逐个 multipart 上传。
   *  重复策略与 importEntries 一致（首问后整批生效） */
  async function importFiles(files: File[]) {
    await runBatch(
      files.map((file) => ({ file, relPath: file.name })),
      () => library.currentFolderPath ?? '',
      '上传',
    );
  }

  /** 批量入库：folderOf 决定每个文件的目标文件夹；文案动词按渠道区分（导入=路径导入，上传=multipart） */
  async function runBatch(files: DroppedFile[], folderOf: (relPath: string) => string, verb: string) {
    await runImportBatch(files, {
      importOne: (f, skipExisting) => {
        const folder = folderOf(f.relPath) || undefined;
        return hasShell
          ? api.itemAddByPath(shell.getPathForFile(f.file), { folder_path: folder, skip_existing: skipExisting })
          : api.itemUpload(f.file, { folder_path: folder, skip_existing: skipExisting });
      },
      askPolicy: askDuplicatePolicy,
      setProgress: (p) => (importProgress.value = p),
      onEmpty: () => library.showToast('未找到可导入的文件'),
      onSummary: (c) =>
        library.showToast(
          `${verb}完成：新增 ${c.added}${c.skipped ? `，忽略重复 ${c.skipped}` : ''}${c.existed ? `，重复导入 ${c.existed}` : ''}${c.failed ? `，失败 ${c.failed}` : ''}`,
        ),
    });
    // SSE item.added 已触发防抖骨架重载，这里不重复拉取
  }

  /** 在 base 下逐级创建目录链（父先子后）；同名已存在静默跳过（合并导入语义），
   *  其余错误也容忍——文件入库阶段 item/add 的 create_dir_all 会兜底，失败项计入 failed */
  async function ensureFolders(dirs: Set<string>, base: string) {
    for (const dir of dirs) {
      const target = joinFolder(base, dir);
      const parent = dirOf(target);
      const name = target.slice(parent ? parent.length + 1 : 0);
      try {
        await api.folderCreate(name, parent || undefined);
      } catch {
        // FILE_EXISTS：目标已有同名目录，按合并导入继续；其他失败交由文件阶段兜底
      }
    }
  }

  /** relPath → 所在目录（无目录部分返回空串） */
  function dirOf(relPath: string): string {
    const idx = relPath.lastIndexOf('/');
    return idx === -1 ? '' : relPath.slice(0, idx);
  }

  /** 库内路径拼接：base 或 sub 为空时直接返回另一方 */
  function joinFolder(base: string, sub: string): string {
    return base && sub ? `${base}/${sub}` : base || sub;
  }

  return { importProgress, dupPrompt, resolveDuplicatePolicy, importBegin, importEntries, importFiles };
});

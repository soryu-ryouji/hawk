// 批量导入共享状态机：importPaths（Electron 拖入路径）与 importFiles（浏览器上传）的唯一差异
// 是「单个文件如何导入」与汇总文案，重复策略（首问后整批生效）、计数、进度推进完全一致。
// 依赖全部经 hooks 注入（api 调用/弹窗/进度/提示），本模块不触碰 store，便于单测。

/** 重复内容策略：ask=首个重复时询问（弹窗对整批生效）；skip=重复一律跳过；import=重复也写入（追加路径副本） */
export type DuplicatePolicy = 'ask' | 'skip' | 'import';

export interface ImportBatchHooks<T> {
  /** 单个文件导入；skipExisting=false 表示重复也写入（服务端缺省即 false，等价于不传） */
  importOne: (item: T, skipExisting: boolean) => Promise<{ skipped: boolean; already_existed: boolean }>;
  /** ask 状态下首个重复时询问整批策略 */
  askPolicy: () => Promise<Exclude<DuplicatePolicy, 'ask'>>;
  /** 进度（null 表示任务结束隐藏指示） */
  setProgress: (p: { total: number; done: number } | null) => void;
  /** 空列表提示（进度已先置 null） */
  onEmpty: () => void;
  /** 结束汇总（进度已先置 null）：新增/重复导入/忽略重复/失败 计数 */
  onSummary: (counts: { added: number; existed: number; skipped: number; failed: number }) => void;
}

export async function runImportBatch<T>(items: T[], hooks: ImportBatchHooks<T>): Promise<void> {
  if (items.length === 0) {
    hooks.setProgress(null);
    hooks.onEmpty();
    return;
  }
  const total = items.length;
  hooks.setProgress({ total, done: 0 });
  let added = 0;
  let existed = 0;
  let skipped = 0;
  let failed = 0;
  let done = 0;
  let policy: DuplicatePolicy = 'ask';
  for (const item of items) {
    try {
      let res = await hooks.importOne(item, policy !== 'import');
      if (res.skipped && policy === 'ask') {
        // 首个重复：暂停问一次，选择对整批生效；选 import 时本项以不跳过重试
        policy = await hooks.askPolicy();
        if (policy === 'import') {
          res = await hooks.importOne(item, false);
        }
      }
      if (res.skipped) {
        skipped++;
      } else if (res.already_existed) {
        existed++;
      } else {
        added++;
      }
    } catch {
      failed++;
    }
    done += 1;
    hooks.setProgress({ total, done });
  }
  hooks.setProgress(null);
  hooks.onSummary({ added, existed, skipped, failed });
}

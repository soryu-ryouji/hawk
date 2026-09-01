// importBatch.ts 批量导入状态机的单元测试：ask → skip / ask → import 两条路径、
// 计数汇总、进度推进序列。hooks 全部用记录桩替代，不触碰 store/api。
import { describe, expect, it } from 'vitest';
import { runImportBatch, type ImportBatchHooks } from './importBatch';

interface Log {
  progress: ({ total: number; done: number } | null)[];
  imported: { item: string; skipExisting: boolean }[];
  asks: number;
  empty: number;
  summary: { added: number; existed: number; skipped: number; failed: number }[];
}

/** 造一批 hooks：queue 为每个文件的导入结果（或抛错），ask 为询问返回值 */
function makeHooks(queue: Array<{ skipped?: boolean; already_existed?: boolean } | 'throw'>, ask: 'skip' | 'import') {
  let cursor = 0;
  const log: Log = { progress: [], imported: [], asks: 0, empty: 0, summary: [] };
  const hooks: ImportBatchHooks<string> = {
    importOne: async (item, skipExisting) => {
      log.imported.push({ item, skipExisting });
      if (cursor >= queue.length) {
        throw new Error('test queue exhausted');
      }
      const next = queue[cursor++];
      if (next === 'throw') {
        throw new Error('boom');
      }
      return { skipped: next?.skipped ?? false, already_existed: next?.already_existed ?? false };
    },
    askPolicy: async () => {
      log.asks++;
      return ask;
    },
    setProgress: (p) => log.progress.push(p),
    onEmpty: () => log.empty++,
    onSummary: (c) => log.summary.push(c),
  };
  return { hooks, log };
}

describe('runImportBatch', () => {
  it('空列表：进度置空 + onEmpty，不触碰 importOne', async () => {
    const { hooks, log } = makeHooks([], 'skip');
    await runImportBatch([], hooks);
    expect(log.progress).toEqual([null]);
    expect(log.empty).toBe(1);
    expect(log.imported).toEqual([]);
    expect(log.summary).toEqual([]);
  });

  it('全部新增：计数与进度序列（0 → 逐项 → null → 汇总）', async () => {
    const { hooks, log } = makeHooks([{}, {}, {}], 'skip');
    await runImportBatch(['a', 'b', 'c'], hooks);
    expect(log.imported).toEqual([
      { item: 'a', skipExisting: true },
      { item: 'b', skipExisting: true },
      { item: 'c', skipExisting: true },
    ]);
    expect(log.progress).toEqual([
      { total: 3, done: 0 },
      { total: 3, done: 1 },
      { total: 3, done: 2 },
      { total: 3, done: 3 },
      null,
    ]);
    expect(log.summary).toEqual([{ added: 3, existed: 0, skipped: 0, failed: 0 }]);
    expect(log.asks).toBe(0);
  });

  it('ask → skip：只问一次，首个重复计入 skipped，后续重复不再询问且仍跳过', async () => {
    // a 新增；b 首个重复（问→skip，仍跳过）；c 重复直接跳过；d 新增
    const { hooks, log } = makeHooks([{}, { skipped: true }, { skipped: true }, {}], 'skip');
    await runImportBatch(['a', 'b', 'c', 'd'], hooks);
    expect(log.asks).toBe(1);
    expect(log.imported).toEqual([
      { item: 'a', skipExisting: true },
      { item: 'b', skipExisting: true },
      { item: 'c', skipExisting: true },
      { item: 'd', skipExisting: true },
    ]);
    expect(log.summary).toEqual([{ added: 2, existed: 0, skipped: 2, failed: 0 }]);
  });

  it('ask → import：首个重复以不跳过重试，后续重复写入，existed 计数', async () => {
    // a 新增；b 首个重复（问→import，不跳过重试得 already_existed）；c 重复直接写入
    const { hooks, log } = makeHooks([{}, { skipped: true }, { already_existed: true }, { already_existed: true }], 'import');
    await runImportBatch(['a', 'b', 'c'], hooks);
    expect(log.asks).toBe(1);
    expect(log.imported).toEqual([
      { item: 'a', skipExisting: true },
      { item: 'b', skipExisting: true },
      { item: 'b', skipExisting: false },
      { item: 'c', skipExisting: false },
    ]);
    expect(log.summary).toEqual([{ added: 1, existed: 2, skipped: 0, failed: 0 }]);
  });

  it('importOne 抛错计入 failed 且不打断后续项', async () => {
    const { hooks, log } = makeHooks([{}, 'throw', {}], 'skip');
    await runImportBatch(['a', 'b', 'c'], hooks);
    expect(log.summary).toEqual([{ added: 2, existed: 0, skipped: 0, failed: 1 }]);
    expect(log.progress.at(-1)).toBeNull();
    expect(log.progress.filter((p) => p !== null)).toHaveLength(4); // 初始 + 3 项
  });

  it('重试后的结果参与计数（ask → import 重试仍 skipped 时不再二次询问）', async () => {
    // b 首问 import，重试仍 skipped（服务端竞态）：按 skipped 计，不问第二次
    const { hooks, log } = makeHooks([{}, { skipped: true }, { skipped: true }, { skipped: true }], 'import');
    await runImportBatch(['a', 'b', 'c'], hooks);
    expect(log.asks).toBe(1);
    expect(log.summary).toEqual([{ added: 1, existed: 0, skipped: 2, failed: 0 }]);
  });
});

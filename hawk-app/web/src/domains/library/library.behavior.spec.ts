// @vitest-environment jsdom
// 特征化测试（refactor-plan.md 阶段 0）：锁定主 store 的编排行为——SSE 就地清理顺序、
// 防抖重载与 hooks 转发、加载版本竞争、写操作消息组装。阶段 3 拆分（写操作 → 服务层）
// 与后续域迁移以本组测试为行为等价依据：SSE/竞争部分不改一字，写操作部分仅改调用来源。
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

const mocks = vi.hoisted(() => ({
  itemSkeleton: vi.fn((): Promise<{ items: import('@/shared/types').SkeletonItem[]; total_size: number }> => Promise.resolve({ items: [], total_size: 0 })),
  itemList: vi.fn(
    (): Promise<{
      items: import('@/shared/types').Item[];
      total: number;
      offset: number;
      limit: number;
      total_size: number;
    }> => Promise.resolve({ items: [], total: 0, offset: 0, limit: 0, total_size: 0 }),
  ),
  itemDetail: vi.fn((): Promise<import('@/shared/types').Item> => Promise.reject(new Error('detail 不应被调用'))),
  itemBatchUpdate: vi.fn(
    (
      _ids: string[],
      _patch: { add_categories?: string[]; add_tags?: string[]; star?: number },
    ): Promise<{ updated: number; missing_ids: string[]; conflicts?: string[] }> => Promise.resolve({ updated: 0, missing_ids: [] }),
  ),
  itemAggregate: vi.fn((_ids: string[]): Promise<{ common_tags: string[]; common_categories: string[] }> =>
    Promise.resolve({ common_tags: [], common_categories: [] }),
  ),
  itemDelete: vi.fn((_id: string, _path?: string): Promise<void> => Promise.resolve()),
  itemRestore: vi.fn((_id: string, _path?: string): Promise<void> => Promise.resolve()),
}));
vi.mock('@/shared/api/endpoints', () => ({ api: mocks }));

import { useLibraryStore, registerTaxonomyHooks } from './store';
import { useTaxonomyStore } from '@/domains/taxonomy';
import { trashSelected, setStarForSelected, addCategoryToSelected } from './actions';
import { itemKey } from './logic/viewLogic';
import type { Item, LibraryInfo, SkeletonItem } from '@/shared/types';

// ---- 夹具 ----

function skel(id: string, path: string, size = 100): SkeletonItem {
  return { id, path, size, star: 0, width: 100, height: 80 };
}

function detail(id: string, path: string, over: Partial<Item> = {}): Item {
  return {
    annotation: null,
    categories: [],
    ext: 'png',
    folders: [],
    height: 80,
    id,
    modification_time: 1,
    name: `${id}.png`,
    palette: [],
    path,
    paths: [path],
    size: 100,
    star: 0,
    tags: [],
    url: null,
    width: 100,
    ...over,
  };
}

const libraryInfo: LibraryInfo = {
  name: '库',
  path: '/lib',
  item_count: 0,
  total_size: 0,
  storage_mode: 'db',
  scan: { periodic: false, interval_seconds: 0 },
} as unknown as LibraryInfo; // 仅锁定 library.updated 分支的写入，字段以事件负载为准

/** 手动 resolve 的延迟响应（竞争测试用） */
function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

const hooks = { refreshTaxonomy: vi.fn(), refreshFolders: vi.fn(), onGlobalFilterChanged: vi.fn(), onLocksChanged: vi.fn(), onLockedView: vi.fn() };

beforeEach(() => {
  setActivePinia(createPinia());
  vi.useFakeTimers();
  for (const fn of Object.values(mocks)) fn.mockClear();
  hooks.refreshTaxonomy.mockClear();
  hooks.refreshFolders.mockClear();
  registerTaxonomyHooks(hooks);
});

afterEach(() => {
  vi.useRealTimers();
});

// ---- SSE 清理与指示 ----

describe('applyEvent 就地清理与指示', () => {
  it('item.trashed 清理该 hash 全部位置（details/skeleton/selection）并防抖重载', async () => {
    const store = useLibraryStore();
    const kA = itemKey('a', 'a.png');
    const kB = itemKey('b', 'b.png');
    store.skeleton = [skel('a', 'a.png'), skel('b', 'b.png')];
    store.details = new Map([
      [kA, detail('a', 'a.png')],
      [kB, detail('b', 'b.png')],
    ]);
    store.selection = [kA, kB];

    store.applyEvent('item.trashed', { id: 'a' });

    expect(store.skeleton.map((s) => s.id)).toEqual(['b']);
    expect([...store.details.keys()]).toEqual([kB]);
    expect(store.selection).toEqual([kB]);
    expect(mocks.itemSkeleton).not.toHaveBeenCalled(); // 防抖期内未发请求
    await vi.advanceTimersByTimeAsync(200);
    expect(mocks.itemSkeleton).toHaveBeenCalledTimes(1); // 兜底重载（含回收站视图进入语义）
  });

  it('task.progress 建立与归零缩略图/索引指示', () => {
    const store = useLibraryStore();
    store.applyEvent('task.progress', { task: 'thumbnail', pending: 2, active: 1 });
    expect(store.taskBacklog).toEqual({ pending: 2, active: 1 });
    store.applyEvent('task.progress', { task: 'thumbnail', pending: 0, active: 0 });
    expect(store.taskBacklog).toBeNull();

    store.applyEvent('task.progress', { task: 'index', pending: 1, active: 0, phase: 'scan', processed: 5, total: 10 });
    expect(store.indexProgress).toEqual({ pending: 1, active: 0, phase: 'scan', processed: 5, total: 10 });
    store.applyEvent('task.progress', { task: 'index', pending: 0, active: 0 });
    expect(store.indexProgress).toBeNull();
  });

  it('library.updated 就地对齐库信息', () => {
    const store = useLibraryStore();
    store.applyEvent('library.updated', { ...libraryInfo, name: '改名后的库' });
    expect(store.library?.name).toBe('改名后的库');
    expect(mocks.itemSkeleton).not.toHaveBeenCalled(); // 不触发列表重载
  });
});

// ---- SSE hooks 转发与防抖 ----

describe('SSE hooks 转发', () => {
  it('items.added 转发 taxonomy 双钩子并防抖重载骨架', async () => {
    const store = useLibraryStore();
    store.applyEvent('items.added', null);
    expect(hooks.refreshTaxonomy).toHaveBeenCalledTimes(1);
    expect(hooks.refreshFolders).toHaveBeenCalledTimes(1);
    expect(mocks.itemSkeleton).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(200);
    expect(mocks.itemSkeleton).toHaveBeenCalledTimes(1);
  });

  it('folder.changed 只刷新目录树，不重载骨架', async () => {
    const store = useLibraryStore();
    store.applyEvent('folder.changed', {});
    expect(hooks.refreshFolders).toHaveBeenCalledTimes(1);
    expect(hooks.refreshTaxonomy).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(200);
    expect(mocks.itemSkeleton).not.toHaveBeenCalled();
  });
});

// ---- applyUpdatedItem 编排（详情合并 / 位置重拉 / 重载判定） ----

describe('applyUpdatedItem 编排', () => {
  it('位置集变化：合并详情、重拉位置级详情、防抖重载', async () => {
    const store = useLibraryStore();
    const kA = itemKey('a', 'a.png');
    store.skeleton = [skel('a', 'a.png')];
    store.details = new Map([[kA, detail('a', 'a.png')]]);
    mocks.itemDetail.mockImplementationOnce(() => Promise.resolve(detail('a', 'moved/b.png')));

    // 事件载荷主位置已变为 b.png（paths 变化 → locationSetChanged）
    store.applyEvent('item.updated', detail('a', 'moved/b.png', { paths: ['moved/b.png'] }));

    expect(mocks.itemDetail).toHaveBeenCalledWith('a', 'a.png'); // 按旧缓存 key 重拉
    expect(store.details.get(kA)?.paths).toEqual(['moved/b.png']); // 内容级字段已合并
    await vi.advanceTimersByTimeAsync(200);
    expect(mocks.itemSkeleton).toHaveBeenCalledTimes(1); // 位置集变化必重载
  });

  it('评分变化：就地补丁骨架与详情，不重载（无过滤视图）', async () => {
    const store = useLibraryStore();
    const kA = itemKey('a', 'a.png');
    store.skeleton = [skel('a', 'a.png')];
    store.details = new Map([[kA, detail('a', 'a.png')]]);

    store.applyEvent('item.updated', detail('a', 'a.png', { star: 5 }));

    expect(store.skeleton[0].star).toBe(5);
    expect(store.details.get(kA)?.star).toBe(5);
    expect(mocks.itemDetail).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(200);
    expect(mocks.itemSkeleton).not.toHaveBeenCalled(); // 骨架已补丁，无需重载
  });

  it('分类维度变化：转发 taxonomy 钩子', () => {
    const store = useLibraryStore();
    const kA = itemKey('a', 'a.png');
    store.skeleton = [skel('a', 'a.png')];
    store.details = new Map([[kA, detail('a', 'a.png')]]);

    store.applyEvent('item.updated', detail('a', 'a.png', { tags: ['新标签'] }));
    expect(hooks.refreshTaxonomy).toHaveBeenCalledTimes(1);
  });
});

// ---- 加载竞争（skeletonVersion / in-flight 合并） ----

describe('加载竞争', () => {
  it('resetList 丢弃过期响应（期间又发起的重载使旧结果作废）', async () => {
    const store = useLibraryStore();
    const d1 = deferred<{ items: SkeletonItem[]; total_size: number }>();
    const d2 = deferred<{ items: SkeletonItem[]; total_size: number }>();
    mocks.itemSkeleton.mockImplementationOnce(() => d1.promise).mockImplementationOnce(() => d2.promise);

    const p1 = store.resetList();
    const p2 = store.resetList();
    d1.resolve({ items: [skel('old', 'old.png')], total_size: 1 });
    d2.resolve({ items: [skel('new', 'new.png')], total_size: 1 });
    await Promise.all([p1, p2]);

    expect(store.skeleton.map((s) => s.id)).toEqual(['new']); // 旧响应被版本守卫丢弃
    expect(store.loading).toBe(false);
  });

  it('ensureWindow 丢弃骨架版本变化后的窗口响应', async () => {
    const store = useLibraryStore();
    store.skeleton = [skel('a', 'a.png'), skel('b', 'b.png')];
    const d = deferred<{ items: Item[]; total: number; offset: number; limit: number; total_size: number }>();
    mocks.itemList.mockImplementationOnce(() => d.promise);

    const win = store.ensureWindow(0, 2);
    // 窗口请求在途时骨架重载（版本自增），窗口结果应作废
    store.skeleton = [skel('a2', 'x.png')];
    await store.reloadSkeleton();
    d.resolve({ items: [detail('z', 'z.png')], total: 1, offset: 0, limit: 2, total_size: 1 });
    await win;

    expect(store.details.size).toBe(0); // z 未入缓存；重载也清掉了旧 key
    expect(store.windowLoading).toBe(false);
  });

  it('reloadSkeleton in-flight 合并：重载进行中再来一轮只补跑一次', async () => {
    const store = useLibraryStore();
    const d1 = deferred<{ items: SkeletonItem[]; total_size: number }>();
    const d2 = deferred<{ items: SkeletonItem[]; total_size: number }>();
    mocks.itemSkeleton.mockImplementationOnce(() => d1.promise).mockImplementationOnce(() => d2.promise);

    void store.reloadSkeleton(); // 第一轮在途
    void store.reloadSkeleton(); // 置脏标记，不并发发请求
    expect(mocks.itemSkeleton).toHaveBeenCalledTimes(1);

    d1.resolve({ items: [], total_size: 0 });
    await vi.advanceTimersByTimeAsync(0); // 微任务推进：第一轮收尾后补跑脏轮
    expect(mocks.itemSkeleton).toHaveBeenCalledTimes(2);
    d2.resolve({ items: [], total_size: 0 });
    await vi.advanceTimersByTimeAsync(0);
    expect(mocks.itemSkeleton).toHaveBeenCalledTimes(2); // 无更多脏标记，不第三轮
  });
});

// ---- 写操作 ----

describe('写操作', () => {
  it('trashSelected 逐位置失败不中断，完成后清选择集并防抖重载', async () => {
    const store = useLibraryStore();
    const kA = itemKey('a', 'a.png');
    const kB = itemKey('b', 'b.png');
    store.skeleton = [skel('a', 'a.png'), skel('b', 'b.png')];
    store.selection = [kA, kB];
    mocks.itemDelete.mockImplementationOnce(() => Promise.reject(new Error('占用中'))).mockImplementationOnce(() => Promise.resolve());

    await trashSelected();

    expect(mocks.itemDelete).toHaveBeenCalledTimes(2); // 第一个失败不中断第二个
    expect(mocks.itemDelete).toHaveBeenNthCalledWith(1, 'a', 'a.png');
    expect(mocks.itemDelete).toHaveBeenNthCalledWith(2, 'b', 'b.png');
    expect(store.selection).toEqual([]);
    expect(store.toast).toBe('Error: 占用中'); // 普通错误走 String(e) 口径（ApiError 才有翻译）
    await vi.advanceTimersByTimeAsync(200);
    expect(mocks.itemSkeleton).toHaveBeenCalledTimes(1);
  });

  it('批量评分：冲突前 3 截断与 missing 计数进 toast', async () => {
    const store = useLibraryStore();
    store.skeleton = [skel('a', 'a.png')];
    store.selection = [itemKey('a', 'a.png')];
    mocks.itemBatchUpdate.mockResolvedValueOnce({
      updated: 1,
      missing_ids: ['m1', 'm2'],
      conflicts: ['a.png', 'b.png', 'c.png', 'd.png'],
    });

    await setStarForSelected(5);

    expect(mocks.itemBatchUpdate).toHaveBeenCalledWith(['a'], { star: 5 });
    expect(store.toast).toBe('已设置评分（a.png、b.png、c.png 等 4 个 因目标文件夹已存在同名文件未移动，2 个未处理）');
  });

  it('addCategoryToSelected：已加载详情中已有该分类的 id 不重复提交', async () => {
    const store = useLibraryStore();
    const kA = itemKey('a', 'a.png');
    const kB = itemKey('b', 'b.png');
    store.skeleton = [skel('a', 'a.png'), skel('b', 'b.png')];
    store.details = new Map([[kA, detail('a', 'a.png', { categories: ['品牌'] })]]);
    store.selection = [kA, kB];
    // 拆分后分类维度刷新走 taxonomy 的防抖公共入口（与 SSE hooks 同一函数），Spy 在调用源上
    const refreshSoon = vi.spyOn(useTaxonomyStore(), 'refreshTaxonomySoon');

    await addCategoryToSelected('品牌');

    expect(mocks.itemBatchUpdate).toHaveBeenCalledWith(['b'], { add_categories: ['品牌'] }); // a 已有该分类被过滤
    expect(refreshSoon).toHaveBeenCalledTimes(1);
  });
});

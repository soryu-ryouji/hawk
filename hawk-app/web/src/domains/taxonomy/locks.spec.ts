// @vitest-environment jsdom
// 锁判定的纯逻辑与流程测试：coveringLock 的祖先链解析（锁条目可以是祖先文件夹）、
// isLocked/isUnlocked 的组合语义（票据持有即解锁）、进入锁定视图的 403 → lockedView
// 锁占位流程与解锁退场。api 层 mock。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

window.matchMedia =
  window.matchMedia ??
  ((query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener() {},
      removeListener() {},
      addEventListener() {},
      removeEventListener() {},
      dispatchEvent: () => false,
    }) as MediaQueryList);

const mocks = vi.hoisted(() => ({
  lockList: vi.fn(() => Promise.resolve({ folders: [], categories: [], tags: [] })),
  lockUnlock: vi.fn(() => Promise.resolve({ unlock_token: 't-test' })),
  itemSkeleton: vi.fn(() => Promise.resolve({ items: [], total_size: 0 })),
}));
vi.mock('@/shared/api/endpoints', () => ({ api: mocks }));
vi.mock('@/shared/lib/platform', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/shared/lib/platform')>();
  return { ...actual, hasShell: false };
});

import { useTaxonomyStore } from '@/domains/taxonomy';
import { useLibraryStore } from '@/domains/library';
import { registerTaxonomyHooks } from '@/domains/library';
import { addUnlockTicket, clearUnlockTickets } from '@/shared/api/client';
import { ApiError } from '@/shared/api/client';

describe('taxonomy 锁判定', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    clearUnlockTickets(); // 票据是模块级 Map，跨用例隔离
  });

  it('coveringLock：文件夹取自身或最近锁定祖先；分类/标签精确匹配', () => {
    const taxonomy = useTaxonomyStore();
    taxonomy.locks.folders = ['posters'];
    taxonomy.locks.categories = ['私密'];
    taxonomy.locks.tags = ['nsfw'];

    // 文件夹：自身命中
    expect(taxonomy.coveringLock('folder', 'posters')).toBe('posters');
    // 子孙路径取最近锁定祖先
    expect(taxonomy.coveringLock('folder', 'posters/2024')).toBe('posters');
    // 无锁路径；前缀字符串相似但非路径祖先
    expect(taxonomy.coveringLock('folder', 'other')).toBeNull();
    expect(taxonomy.coveringLock('folder', 'po')).toBeNull();

    // 分类/标签：精确匹配
    expect(taxonomy.coveringLock('category', '私密')).toBe('私密');
    expect(taxonomy.coveringLock('category', '其他')).toBeNull();
    expect(taxonomy.coveringLock('tag', 'nsfw')).toBe('nsfw');
  });

  it('isLocked / isUnlocked：票据持有即解锁，且按覆盖锁条目判定', () => {
    const taxonomy = useTaxonomyStore();
    taxonomy.locks.folders = ['private'];

    // 覆盖但未解锁
    expect(taxonomy.isLocked('folder', 'private/photos')).toBe(true);
    expect(taxonomy.isUnlocked('folder', 'private/photos')).toBe(false);

    // 解锁锁条目（而非子路径）：子孙随之解锁
    addUnlockTicket('folder', 'private', 't1');
    expect(taxonomy.isUnlocked('folder', 'private/photos')).toBe(true);

    // 未锁路径：isLocked false，isUnlocked false（无锁可解）
    expect(taxonomy.isLocked('folder', 'public')).toBe(false);
    expect(taxonomy.isUnlocked('folder', 'public')).toBe(false);
  });

  it('进入锁定文件夹视图：查询 403 → lockedView 锁占位（不弹框、不导航回退），覆盖锁为祖先', async () => {
    const library = useLibraryStore();
    const taxonomy = useTaxonomyStore();
    // 装配主 store 的 hooks（App 层 boot 时由 taxonomy store 注册，测试内手动等效）
    registerTaxonomyHooks({
      refreshTaxonomy: () => {},
      refreshFolders: () => {},
      onGlobalFilterChanged: () => {},
      onLocksChanged: () => {},
      onLockedView: () => {
        const v = library.view;
        if (v.kind === 'folder') {
          const entry = taxonomy.coveringLock('folder', v.path);
          if (entry) library.setLockedView({ dimension: 'folder', name: entry });
        }
      },
    });

    taxonomy.locks.folders = ['private'];
    // 骨架查询被服务端以 403 LOCKED 拒绝
    mocks.itemSkeleton.mockRejectedValueOnce(new ApiError('LOCKED', '已锁定', 403));

    // 点击锁定文件夹：导航照常发生（可见性 A），不弹任何框
    library.setView({ kind: 'folder', path: 'private/photos' });
    await vi.waitFor(() => {
      // 内容区切锁占位：lockedView 指向覆盖的锁条目（祖先 private）
      expect(library.lockedView).toEqual({ dimension: 'folder', name: 'private' });
    });
    expect(library.view).toEqual({ kind: 'folder', path: 'private/photos' }); // 导航不回退
    expect(library.skeleton).toEqual([]); // 残留内容已清空
  });

  it('解锁成功：lockedView 退场并原地重查当前视图', async () => {
    const library = useLibraryStore();
    const taxonomy = useTaxonomyStore();
    taxonomy.locks.folders = ['private'];
    library.setLockedView({ dimension: 'folder', name: 'private' });

    const ok = await taxonomy.unlock('folder', 'private', 'pw');
    expect(ok).toBe(true);
    // 解锁后锁占位立即退场；票据已登记
    expect(library.lockedView).toBeNull();
    expect(taxonomy.isUnlocked('folder', 'private/x')).toBe(true);
  });
});

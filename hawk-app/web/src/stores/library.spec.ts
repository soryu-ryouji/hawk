// @vitest-environment jsdom
// 主 store 的视图导航与排序偏好行为：历史栈语义、视图记忆与偏好持久化。
// 这组行为在 store 拆分子模块（libraryNavigation）时最容易回归，先固化再重构。
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';

const mocks = vi.hoisted(() => ({
  itemSkeleton: vi.fn(() => Promise.resolve({ items: [], total_size: 0 })),
  itemList: vi.fn(() => Promise.resolve({ items: [], total: 0, offset: 0, limit: 0, total_size: 0 })),
  viewPreferences: vi.fn(() => Promise.resolve({})),
  viewPreferenceSet: vi.fn(() => Promise.resolve()),
  viewPreferenceReset: vi.fn(() => Promise.resolve()),
}));
vi.mock('../api/endpoints', () => ({ api: mocks }));

import { useLibraryStore } from './library';

beforeEach(() => {
  setActivePinia(createPinia());
  // 该 vitest 的 jsdom 不提供 localStorage（persist.ts 已按缺失兜底），有则清空
  (globalThis as { localStorage?: Storage }).localStorage?.clear();
  for (const fn of Object.values(mocks)) {
    fn.mockClear();
  }
});

describe('视图历史', () => {
  it('setView 压栈，goBack/goForward 在栈内移动', () => {
    const store = useLibraryStore();
    store.setView({ kind: 'folder', path: 'A' });
    store.setView({ kind: 'category', name: 'C' });
    expect(store.view).toEqual({ kind: 'category', name: 'C' });
    expect(store.canGoBack).toBe(true);
    expect(store.canGoForward).toBe(false);

    store.goBack();
    expect(store.view).toEqual({ kind: 'folder', path: 'A' });
    expect(store.canGoForward).toBe(true);

    store.goForward();
    expect(store.view).toEqual({ kind: 'category', name: 'C' });
  });

  it('历史中间切换视图截断前进分支', () => {
    const store = useLibraryStore();
    store.setView({ kind: 'folder', path: 'A' });
    store.setView({ kind: 'folder', path: 'B' });
    store.goBack();
    store.setView({ kind: 'tag', name: 'T' });
    expect(store.canGoForward).toBe(false);

    store.goForward(); // 无前进项：原地不动
    expect(store.view).toEqual({ kind: 'tag', name: 'T' });
  });

  it('correctView 就地修正当前条目，不新增历史', () => {
    const store = useLibraryStore();
    store.setView({ kind: 'folder', path: 'A' });
    store.correctView({ kind: 'folder', path: 'A2' });
    expect(store.view).toEqual({ kind: 'folder', path: 'A2' });

    store.goBack(); // 历史仅一条：不移动
    expect(store.view).toEqual({ kind: 'folder', path: 'A2' });
  });
});

describe('排序偏好', () => {
  it('setQuery 按当前视图 scope 持久化排序', () => {
    const store = useLibraryStore();
    store.setView({ kind: 'folder', path: 'A' });
    store.setQuery({ orderBy: 'name', order: 'asc' });
    expect(store.viewPrefs['folder:A']).toEqual({ order_by: 'name', order: 'asc' });
    expect(mocks.viewPreferenceSet).toHaveBeenCalledWith('folder:A', 'name', 'asc');
  });

  it('resetSort 删除当前 scope 偏好', () => {
    const store = useLibraryStore();
    store.setView({ kind: 'folder', path: 'A' });
    store.setQuery({ orderBy: 'name', order: 'asc' });
    store.resetSort();
    expect(store.viewPrefs['folder:A']).toBeUndefined();
    expect(mocks.viewPreferenceReset).toHaveBeenCalledWith('folder:A');
  });

  it('无排序记忆语义的视图（全部素材）不写偏好', () => {
    const store = useLibraryStore();
    store.setQuery({ orderBy: 'name', order: 'asc' });
    expect(mocks.viewPreferenceSet).not.toHaveBeenCalled();
  });
});

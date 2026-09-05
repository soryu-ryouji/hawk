// viewLogic.ts 纯函数的单元测试（SSE 策略/排序继承/选择集变更的决策逻辑）。
// 与源码同目录放置（web/src/*.spec.ts），`npm run test:unit` 收集。
import { describe, expect, it } from 'vitest';
import {
  DEFAULT_SORT,
  isGlobalViewKind,
  indexSkeletonById,
  isUnfilteredView,
  commonFoldersOf,
  commonStarOf,
  itemKey,
  locationSetChangedOf,
  mergeDetailOnUpdate,
  nextSelection,
  patchSkeletonOnUpdate,
  resolveSort,
  sameNameSet,
  shouldReloadOnUpdate,
  selectionTotalSize,
  skeletonNeedsPatch,
  splitKey,
  taxonomyChanged,
} from './viewLogic';
import type { Item, QueryState, ViewPrefs, ViewState } from './types';

const query = (patch: Partial<QueryState> = {}): QueryState => ({
  keywords: [],
  orderBy: 'modification_time',
  order: 'desc',
  ...patch,
});

describe('sameNameSet', () => {
  it('无序列表相等', () => {
    expect(sameNameSet(['a', 'b'], ['b', 'a'])).toBe(true);
  });

  it('长度不同不相等', () => {
    expect(sameNameSet(['a'], ['a', 'b'])).toBe(false);
  });

  it('元素不同不相等', () => {
    expect(sameNameSet(['a'], ['b'])).toBe(false);
  });

  it('undefined 按空集处理', () => {
    expect(sameNameSet(undefined, undefined)).toBe(true);
    expect(sameNameSet(undefined, [])).toBe(true);
    expect(sameNameSet(undefined, ['a'])).toBe(false);
    expect(sameNameSet(['a'], undefined)).toBe(false);
  });
});

describe('isUnfilteredView', () => {
  it('全部素材且无筛选条件时成立', () => {
    expect(isUnfilteredView({ kind: 'all' }, query())).toBe(true);
  });

  it('任一筛选条件（关键词/评分/颜色）不成立', () => {
    const v: ViewState = { kind: 'all' };
    expect(isUnfilteredView(v, query({ keywords: ['x'] }))).toBe(false);
    expect(isUnfilteredView(v, query({ star: 3 }))).toBe(false);
    expect(isUnfilteredView(v, query({ color: '#fff' }))).toBe(false);
  });

  it('非 all 视图一律不成立', () => {
    expect(isUnfilteredView({ kind: 'folder', path: 'a/b' }, query())).toBe(false);
    expect(isUnfilteredView({ kind: 'trash' }, query())).toBe(false);
    expect(isUnfilteredView({ kind: 'category', name: 'c' }, query())).toBe(false);
  });
});

describe('itemKey / splitKey', () => {
  it('组合与拆分互逆', () => {
    const key = itemKey('abc123', '海报/cat.png');
    expect(splitKey(key)).toEqual({ id: 'abc123', path: '海报/cat.png' });
  });

  it('同 id 不同 path 的 key 不同', () => {
    expect(itemKey('h', 'a.png')).not.toBe(itemKey('h', 'b.png'));
  });
});

describe('resolveSort', () => {
  const prefs: ViewPrefs = {
    'folder:a': { order_by: 'name', order: 'asc' },
    'folder:': { order_by: 'size', order: 'desc' },
    'category:c': { order_by: 'star', order: 'desc' },
    'tag:t': { order_by: 'name', order: 'asc' },
  };

  it('category/tag 直落自身偏好', () => {
    expect(resolveSort({ kind: 'category', name: 'c' }, prefs)).toEqual({ orderBy: 'star', order: 'desc' });
    expect(resolveSort({ kind: 'tag', name: 't' }, prefs)).toEqual({ orderBy: 'name', order: 'asc' });
  });

  it('folder 沿父链继承：自身 → 父级 → 根目录', () => {
    expect(resolveSort({ kind: 'folder', path: 'a' }, prefs)).toEqual({ orderBy: 'name', order: 'asc' });
    expect(resolveSort({ kind: 'folder', path: 'a/b/c' }, prefs)).toEqual({ orderBy: 'name', order: 'asc' });
    expect(resolveSort({ kind: 'folder', path: 'x' }, prefs)).toEqual({ orderBy: 'size', order: 'desc' });
  });

  it('无记忆时回落全局默认', () => {
    expect(resolveSort({ kind: 'folder', path: 'x' }, {})).toEqual(DEFAULT_SORT);
    expect(resolveSort({ kind: 'category', name: 'none' }, prefs)).toEqual(DEFAULT_SORT);
    expect(resolveSort({ kind: 'all' }, prefs)).toEqual(DEFAULT_SORT);
    expect(resolveSort({ kind: 'trash' }, prefs)).toEqual(DEFAULT_SORT);
  });

  it('folder 记忆优先级高于根目录默认（自底向上先命中）', () => {
    const chained: ViewPrefs = {
      'folder:a/b': { order_by: 'star', order: 'asc' },
      'folder:a': { order_by: 'name', order: 'desc' },
    };
    expect(resolveSort({ kind: 'folder', path: 'a/b/c' }, chained)).toEqual({ orderBy: 'star', order: 'asc' });
    expect(resolveSort({ kind: 'folder', path: 'a/x' }, chained)).toEqual({ orderBy: 'name', order: 'desc' });
  });
});

describe('skeletonNeedsPatch', () => {
  const sk = { id: '1', path: 'a.png', width: 100, height: 50, star: 0, size: 10 };
  const item = { id: '1', width: 100, height: 50, star: 0 } as Item;

  it('star 或宽高任一变化才需要替换', () => {
    expect(skeletonNeedsPatch(sk, { ...item, star: 3 })).toBe(true);
    expect(skeletonNeedsPatch(sk, { ...item, width: 200 })).toBe(true);
    expect(skeletonNeedsPatch(sk, { ...item, height: 60 })).toBe(true);
  });

  it('三者都不变则不需要（高频 updated 不重渲染）', () => {
    expect(skeletonNeedsPatch(sk, item)).toBe(false);
  });
});

describe('nextSelection', () => {
  // 条目以 itemKey(id, path) 标识；c/d 同 id 不同 path（同内容多位置）也是独立成员
  const skeleton = [
    { id: 'a', path: 'a.png' },
    { id: 'b', path: 'b.png' },
    { id: 'x', path: 'c.png' },
    { id: 'x', path: 'd.png' },
    { id: 'e', path: 'e.png' },
  ];
  const k = (id: string, path: string) => itemKey(id, path);

  it('默认单选替换', () => {
    expect(nextSelection(skeleton, [k('a', 'a.png'), k('b', 'b.png')], k('x', 'c.png'))).toEqual([k('x', 'c.png')]);
  });

  it('toggle：未选中加入，已选中移除', () => {
    expect(nextSelection(skeleton, [k('a', 'a.png')], k('b', 'b.png'), 'toggle')).toEqual([k('a', 'a.png'), k('b', 'b.png')]);
    expect(nextSelection(skeleton, [k('a', 'a.png'), k('b', 'b.png')], k('a', 'a.png'), 'toggle')).toEqual([k('b', 'b.png')]);
  });

  it('range：以末位选中为锚点框选（向后，含同 id 多位置）', () => {
    expect(nextSelection(skeleton, [k('b', 'b.png')], k('x', 'd.png'), 'range')).toEqual([k('b', 'b.png'), k('x', 'c.png'), k('x', 'd.png')]);
  });

  it('range：向前框选取索引区间', () => {
    expect(nextSelection(skeleton, [k('x', 'd.png')], k('b', 'b.png'), 'range')).toEqual([k('b', 'b.png'), k('x', 'c.png'), k('x', 'd.png')]);
  });

  it('range：空选择集回落单选', () => {
    expect(nextSelection(skeleton, [], k('x', 'c.png'), 'range')).toEqual([k('x', 'c.png')]);
  });

  it('range：锚点不在骨架中回落单选', () => {
    expect(nextSelection(skeleton, ['zzz'], k('x', 'c.png'), 'range')).toEqual([k('x', 'c.png')]);
  });

  it('range：目标不在骨架中回落单选', () => {
    expect(nextSelection(skeleton, [k('b', 'b.png')], 'zzz', 'range')).toEqual(['zzz']);
  });
});

// ---- item.updated 合并/重载决策 ----

const item = (patch: Partial<Item> = {}): Item => ({
  id: 'x',
  name: 'x',
  ext: 'png',
  path: 'x.png',
  paths: ['x.png'],
  folders: [''],
  size: 1,
  width: 8,
  height: 8,
  modification_time: 1,
  star: 0,
  tags: [],
  categories: [],
  palette: [],
  ...patch,
});

const skel = (id: string, path: string, patch: Partial<{ star: number; width: number; height: number; size: number }> = {}) => ({
  id,
  path,
  width: 8,
  height: 8,
  star: 0,
  size: 100,
  ...patch,
});

describe('commonFoldersOf', () => {
  it('全部同目录返回该目录；跨目录返回空；库根为 ""', () => {
    expect(commonFoldersOf([itemKey('a', 'd/1.png'), itemKey('b', 'd/2.png')])).toEqual(['d']);
    expect(commonFoldersOf([itemKey('a', 'd/1.png'), itemKey('b', 'e/2.png')])).toEqual([]);
    expect(commonFoldersOf([itemKey('a', '1.png')])).toEqual(['']);
    expect(commonFoldersOf([])).toEqual([]);
  });
});

describe('commonStarOf', () => {
  it('全同分返回该分；混分/空选/无骨架返回 null', () => {
    const skeleton = [skel('a', '1.png', { star: 3 }), skel('b', '2.png', { star: 3 }), skel('c', '3.png', { star: 5 })];
    expect(commonStarOf([itemKey('a', '1.png'), itemKey('b', '2.png')], skeleton)).toBe(3);
    expect(commonStarOf([itemKey('a', '1.png'), itemKey('c', '3.png')], skeleton)).toBeNull();
    expect(commonStarOf([], skeleton)).toBeNull();
  });
});

describe('selectionTotalSize', () => {
  it('按条目 key 累加骨架 size；未命中按 0', () => {
    const sizes = new Map([[itemKey('x', 'a.png'), 100], [itemKey('x', 'b.png'), 200]]);
    expect(selectionTotalSize([itemKey('x', 'a.png'), itemKey('x', 'b.png')], sizes)).toBe(300);
    expect(selectionTotalSize([itemKey('x', 'a.png'), itemKey('ghost', 'g.png')], sizes)).toBe(100);
    expect(selectionTotalSize([], sizes)).toBe(0);
  });
});

describe('isGlobalViewKind', () => {
  it('全局类视图成立', () => {
    for (const kind of ['all', 'root', 'uncategorized', 'untagged'] as const) {
      expect(isGlobalViewKind({ kind })).toBe(true);
    }
  });

  it('维度自身视图与回收站不成立', () => {
    expect(isGlobalViewKind({ kind: 'folder', path: 'a' })).toBe(false);
    expect(isGlobalViewKind({ kind: 'category', name: 'c' })).toBe(false);
    expect(isGlobalViewKind({ kind: 'tag', name: 't' })).toBe(false);
    expect(isGlobalViewKind({ kind: 'trash' })).toBe(false);
  });
});

describe('locationSetChangedOf', () => {
  it('位置集一致不变化', () => {
    expect(locationSetChangedOf(['a.png'], item({ paths: ['a.png'] }))).toBe(false);
  });

  it('位置增减/改名命中', () => {
    expect(locationSetChangedOf(['a.png'], item({ paths: ['a.png', 'b.png'] }))).toBe(true);
    expect(locationSetChangedOf(['a.png'], item({ paths: ['b.png'] }))).toBe(true);
  });

  it('骨架不含该 hash 时不判定变化（交由非成员分支处理）', () => {
    expect(locationSetChangedOf([], item({ paths: ['a.png'] }))).toBe(false);
  });

  it('回收站口径：骨架 path（含 trash 前缀）剥前缀后与事件 paths 对齐', () => {
    expect(locationSetChangedOf(['.hawk/trash/a.png'], item({ paths: ['a.png'] }))).toBe(false);
  });
});

describe('indexSkeletonById', () => {
  it('同 id 多位置聚为一组下标', () => {
    const map = indexSkeletonById([skel('x', 'a.png'), skel('y', 'b.png'), skel('x', 'c.png')]);
    expect(map.get('x')).toEqual([0, 2]);
    expect(map.get('y')).toEqual([1]);
    expect(map.get('zz')).toBeUndefined();
  });
});

describe('taxonomyChanged', () => {
  it('标签/分类任一变化即命中', () => {
    expect(taxonomyChanged(item({ tags: ['a'] }), item({ tags: ['a'] }))).toBe(false);
    expect(taxonomyChanged(item({ tags: ['a'] }), item({ tags: ['b'] }))).toBe(true);
    expect(taxonomyChanged(item({ categories: [] }), item({ categories: ['c'] }))).toBe(true);
  });
});

describe('mergeDetailOnUpdate', () => {
  it('事件位置条目整体替换', () => {
    const updated = item({ path: 'a.png', name: '新名', tags: ['t'] });
    expect(mergeDetailOnUpdate(item({ path: 'a.png' }), updated, true)).toBe(updated);
  });

  it('其余位置同步内容级字段、保留位置级字段', () => {
    const prev = item({ path: 'b.png', name: 'b位名', size: 99, star: 1 });
    const updated = item({ path: 'a.png', name: 'a位名', size: 5, star: 5, tags: ['t'] });
    const merged = mergeDetailOnUpdate(prev, updated, false);
    expect(merged.name).toBe('b位名');
    expect(merged.size).toBe(99);
    expect(merged.star).toBe(5);
    expect(merged.tags).toEqual(['t']);
  });
});

describe('patchSkeletonOnUpdate', () => {
  it('按索引补丁 star/宽高，不动其他条目', () => {
    const skeleton = [skel('x', 'a.png'), skel('x', 'b.png'), skel('y', 'c.png')];
    const { next, changed } = patchSkeletonOnUpdate(skeleton, [0, 1], item({ id: 'x', star: 3, width: 16, height: 16 }));
    expect(changed).toBe(true);
    expect(next[0]).toMatchObject({ star: 3, width: 16 });
    expect(next[1]).toMatchObject({ star: 3, width: 16 });
    expect(next[2]).toMatchObject({ star: 0 });
  });

  it('无变化返回原数组引用', () => {
    const skeleton = [skel('x', 'a.png')];
    const { next, changed } = patchSkeletonOnUpdate(skeleton, [0], item({ id: 'x' }));
    expect(changed).toBe(false);
    expect(next).toBe(skeleton);
  });
});

describe('shouldReloadOnUpdate', () => {
  const base = {
    locationSetChanged: false,
    inSkeleton: true,
    skeletonChanged: false,
    taxonomyChanged: false,
    unfiltered: true,
    exclusionActive: false,
    single: true,
  };

  it('无变化成员不重载', () => {
    expect(shouldReloadOnUpdate(base)).toBe(false);
  });

  it('位置集变化必重载', () => {
    expect(shouldReloadOnUpdate({ ...base, locationSetChanged: true })).toBe(true);
  });

  it('隐藏排除激活时分类维度变化重载', () => {
    expect(shouldReloadOnUpdate({ ...base, taxonomyChanged: true, exclusionActive: true })).toBe(true);
    expect(shouldReloadOnUpdate({ ...base, taxonomyChanged: true })).toBe(false);
  });

  it('非成员：无过滤视图仅单条事件重拉；过滤视图批量也重拉', () => {
    expect(shouldReloadOnUpdate({ ...base, inSkeleton: false })).toBe(true);
    expect(shouldReloadOnUpdate({ ...base, inSkeleton: false, single: false })).toBe(false);
    expect(shouldReloadOnUpdate({ ...base, inSkeleton: false, single: false, unfiltered: false })).toBe(true);
  });

  it('骨架已就地补丁的非成员不重载（skeletonChanged 蕴含在骨架中，防御分支）', () => {
    expect(shouldReloadOnUpdate({ ...base, inSkeleton: false, skeletonChanged: true, single: false, unfiltered: false })).toBe(false);
  });
});

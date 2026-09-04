// viewLogic.ts 纯函数的单元测试（SSE 策略/排序继承/选择集变更的决策逻辑）。
// 与源码同目录放置（web/src/*.spec.ts），`npm run test:unit` 收集。
import { describe, expect, it } from 'vitest';
import {
  DEFAULT_SORT,
  isUnfilteredView,
  itemKey,
  nextSelection,
  resolveSort,
  sameNameSet,
  skeletonNeedsPatch,
  splitKey,
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
  const sk = { id: '1', path: 'a.png', width: 100, height: 50, star: 0 };
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

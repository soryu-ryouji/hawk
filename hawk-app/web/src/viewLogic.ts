// 视图/查询相关的纯决策逻辑（从 stores/library.ts 抽出以便单测；store 内只做状态接线）。
// 全部为纯函数：不触碰响应式状态，调用方传入当前 state 并消化返回值。
import type { Item, QueryState, SkeletonItem, ViewPrefs, ViewState } from './types';

/** 全局默认排序（无任何记忆时的回落） */
export const DEFAULT_SORT: Pick<QueryState, 'orderBy' | 'order'> = { orderBy: 'modification_time', order: 'desc' };

/** 名称集合相等比较（标签/分类为无序去重列表） */
export function sameNameSet(a: string[] | undefined, b: string[] | undefined): boolean {
  const x = a ?? [];
  const y = b ?? [];
  return x.length === y.length && x.every((v) => y.includes(v));
}

/** 无过滤的「全部素材」视图：item.updated 不可能改变成员资格（进出回收站有独立事件），可原地更新 */
export function isUnfilteredView(view: ViewState, query: QueryState): boolean {
  return view.kind === 'all' && query.keywords.length === 0 && query.star === undefined && !query.color;
}

/** 条目 key：同内容（同 hash）多位置在视图中各自成条，前端以 `${id}\n${path}` 唯一定位。
 *  \n 不可能是文件名字符；DOM dataset / JSON 均安全 */
export function itemKey(id: string, path: string): string {
  return `${id}\n${path}`;
}

/** 回收站位置前缀（与后端 LibraryPaths 一致）：位置路径的展示口径要剥掉它（恢复原路径） */
export const TRASH_PREFIX = '.hawk/trash/';

/** 位置路径的展示口径：回收站位置剥掉 trash 前缀，其余原样 */
export function displayPath(p: string): string {
  return p.startsWith(TRASH_PREFIX) ? p.slice(TRASH_PREFIX.length) : p;
}

/** itemKey 的逆运算：拆出内容 id 与库内位置（API 边界用：写操作按 id+path 寻址） */
export function splitKey(key: string): { id: string; path: string } {
  const i = key.indexOf('\n');
  return { id: key.slice(0, i), path: key.slice(i + 1) };
}

/**
 * 解析视图的有效排序：folder 自底向上沿父链继承（子文件夹自己的设置优先），
 * category/tag 无层级直接回落默认；无记忆语义的视图用全局默认
 */
export function resolveSort(v: ViewState, prefs: ViewPrefs): Pick<QueryState, 'orderBy' | 'order'> {
  const hit = (scope: string) => {
    const e = prefs[scope];
    return e ? { orderBy: e.order_by, order: e.order } : undefined;
  };

  if (v.kind === 'category' || v.kind === 'tag') {
    return hit(`${v.kind}:${v.name}`) ?? DEFAULT_SORT;
  }

  if (v.kind === 'folder') {
    for (let dir = v.path; ; dir = dir.includes('/') ? dir.slice(0, dir.lastIndexOf('/')) : '') {
      const h = hit(`folder:${dir}`);
      if (h) return h;
      if (dir === '') break;
    }
  }

  return DEFAULT_SORT;
}

/** 骨架就地合并判定：star 变化影响徽章，宽高变化（0 × 0 自愈修复）影响布局比例；任一变化才替换，
 *  避免高频 updated 事件触发无谓重渲染 */
export function skeletonNeedsPatch(prev: SkeletonItem, updated: Item): boolean {
  return prev.star !== updated.star || prev.width !== updated.width || prev.height !== updated.height;
}

/** 选择集变更：默认单选替换；toggle 反选；range 以末位选中为锚点框选骨架索引区间（双向）。
 *  条目以 itemKey（id+path）标识：同内容多位置在选择集中是独立成员 */
export function nextSelection(
  skeleton: readonly Pick<SkeletonItem, 'id' | 'path'>[],
  selection: readonly string[],
  key: string,
  mod?: 'range' | 'toggle',
): string[] {
  if (mod === 'range' && selection.length > 0) {
    const anchor = selection[selection.length - 1];
    const a = skeleton.findIndex((i) => itemKey(i.id, i.path) === anchor);
    const b = skeleton.findIndex((i) => itemKey(i.id, i.path) === key);
    if (a >= 0 && b >= 0) {
      const [from, to] = a < b ? [a, b] : [b, a];
      return skeleton.slice(from, to + 1).map((i) => itemKey(i.id, i.path));
    }
  }
  if (mod === 'toggle') {
    return selection.includes(key) ? selection.filter((s) => s !== key) : [...selection, key];
  }
  return [key];
}

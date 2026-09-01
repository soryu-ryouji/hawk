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

/** 视图位置范围：folder 视图 → "<path>/" 前缀，root 视图 → ""（顶层文件）；
 *  其余视图无位置语义（返回 null，「仅从此处移除」选项不适用） */
export function viewPathPrefix(view: ViewState): string | null {
  if (view.kind === 'folder') return `${view.path}/`;
  if (view.kind === 'root') return '';
  return null;
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

/** 选择集变更：默认单选替换；toggle 反选；range 以末位选中为锚点框选骨架索引区间（双向） */
export function nextSelection(
  skeleton: readonly Pick<SkeletonItem, 'id'>[],
  selection: readonly string[],
  id: string,
  mod?: 'range' | 'toggle',
): string[] {
  if (mod === 'range' && selection.length > 0) {
    const anchor = selection[selection.length - 1];
    const a = skeleton.findIndex((i) => i.id === anchor);
    const b = skeleton.findIndex((i) => i.id === id);
    if (a >= 0 && b >= 0) {
      const [from, to] = a < b ? [a, b] : [b, a];
      return skeleton.slice(from, to + 1).map((i) => i.id);
    }
  }
  if (mod === 'toggle') {
    return selection.includes(id) ? selection.filter((s) => s !== id) : [...selection, id];
  }
  return [id];
}

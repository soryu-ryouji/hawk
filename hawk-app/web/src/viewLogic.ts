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

/** 全局类视图（隐藏排除生效的视图）：全部素材/根目录/未分类/未标签；维度自身视图与回收站不排除 */
export function isGlobalViewKind(view: ViewState): boolean {
  return view.kind === 'all' || view.kind === 'root' || view.kind === 'uncategorized' || view.kind === 'untagged';
}

/** item.updated 的位置集变化判定：以骨架为准（details 可能未缓存该 hash）。骨架 path 与事件 paths
 *  同口径化后比对（回收站视图事件 paths 为原路径投影，骨架 path 剥掉 trash 前缀）；
 *  改名/移动/增删位置都会命中 */
export function locationSetChangedOf(
  skeleton: readonly Pick<SkeletonItem, 'id' | 'path'>[],
  updated: Pick<Item, 'id' | 'paths'>,
): boolean {
  const skelPaths = skeleton.filter((s) => s.id === updated.id).map((s) => displayPath(s.path));
  return skelPaths.length > 0 && !sameNameSet(skelPaths, updated.paths);
}

/** item.updated 的分类/标签维度变化判定（内容级，与位置无关） */
export function taxonomyChanged(
  prev: Pick<Item, 'tags' | 'categories'>,
  updated: Pick<Item, 'tags' | 'categories'>,
): boolean {
  return !sameNameSet(prev.tags, updated.tags) || !sameNameSet(prev.categories, updated.categories);
}

/** item.updated 的详情合并：同 hash 全部位置条目同步内容级字段；事件载荷对应的位置条目整体替换
 * （位置级字段 name/ext/size/mtime 仅该条目随全量替换生效），其余位置保留自己的位置级字段 */
export function mergeDetailOnUpdate(prev: Item, updated: Item, isEventLocation: boolean): Item {
  if (isEventLocation) {
    return updated;
  }
  return {
    ...prev,
    tags: updated.tags,
    categories: updated.categories,
    star: updated.star,
    annotation: updated.annotation,
    url: updated.url,
    width: updated.width,
    height: updated.height,
    palette: updated.palette,
    paths: updated.paths,
    folders: updated.folders,
  };
}

/** item.updated 的骨架就地合并：该 hash 全部位置条目同步 star/宽高（★ 角标与布局比例）；
 *  未变化返回原数组引用（changed=false），调用方据此跳过重渲染 */
export function patchSkeletonOnUpdate(
  skeleton: readonly SkeletonItem[],
  updated: Item,
): { next: SkeletonItem[]; changed: boolean } {
  let changed = false;
  const next = skeleton.map((s) => {
    if (s.id !== updated.id || !skeletonNeedsPatch(s, updated)) {
      return s;
    }
    changed = true;
    return { ...s, star: updated.star, width: updated.width, height: updated.height };
  });
  return { next: changed ? next : (skeleton as SkeletonItem[]), changed };
}

/** item.updated 后是否需要重载骨架（成员/次序以服务端查询为准的兜底时机）。
 *  三种重载理由：位置集变化（新位置卡片出现/消失）；隐藏排除激活时的分类维度变化
 * （挂上/摘掉隐藏维度即进出全局视图）；不属于当前骨架且视图可能漏新成员（过滤视图或单条事件） */
export function shouldReloadOnUpdate(args: {
  locationSetChanged: boolean;
  inSkeleton: boolean;
  skeletonChanged: boolean;
  taxonomyChanged: boolean;
  /** 无过滤视图（全部素材 + 无关键词/评分/颜色）：item.updated 不可能改变成员资格 */
  unfiltered: boolean;
  /** 隐藏排除激活（全局类视图且隐藏集非空）：分类/标签变化可能改变成员资格 */
  exclusionActive: boolean;
  /** 单条事件（用户操作规模）才为非成员重拉；批量事件（调色板回写）不重拉，避免风暴 */
  single: boolean;
}): boolean {
  if (args.locationSetChanged) {
    return true;
  }
  if (args.taxonomyChanged && args.exclusionActive) {
    return true;
  }
  return !args.skeletonChanged && !args.inSkeleton && (!args.unfiltered || args.single);
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

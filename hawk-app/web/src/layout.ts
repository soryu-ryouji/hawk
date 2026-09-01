// 网格布局的共享常量与算法（ItemGrid 的行布局数学 + ItemCard 的卡片样式共同消费）。
// 单一来源：改这里即全局生效，ItemCard 通过根元素 CSS 变量接收（避免「两处必须一致」的隐式耦合）。
// 齐行布局纯函数（layoutRows）与本文件同处，避免文件碎片化。

/** 行间距（ItemGrid .row 的 gap 与行推进共用） */
export const GRID_GAP = 10;

/** 卡片 meta 区定高（Eagle 式 3 行：标题 2 + 像素 1）：行距按它计算，ItemCard 的 .meta 经 --meta-h 消费 */
export const CARD_META_H = 54;

/** 卡片总边框宽（2px × 2）：行槽位必须计入，否则下一行图片盖住上一行的 meta 文字；ItemCard 经 --card-border 消费 */
export const CARD_BORDER = 4;

/** 布局输入的骨架最小形状（store 骨架经 Number 强转后传入） */
export interface SkeletonLike {
  id: string;
  width: number;
  height: number;
  star: number;
}

export interface LayoutCell {
  id: string;
  width: number;
  height: number;
  star: number;
}

export interface LayoutRow {
  key: string;
  cells: LayoutCell[];
  y: number;
  height: number;
  /** 行内条目在骨架中的索引区间 [startIdx, endIdx)，视口窗口按它向 store 补数据 */
  startIdx: number;
  endIdx: number;
}

/**
 * 齐行布局（justified layout）：贪心装行，累计到超出容器即切行；非末行按容器宽精确反推行高。
 * 只依赖骨架 + 卡片尺寸 + 容器宽：详情缓存变化不触发全量重排（大库上每次窗口拉取都重排太贵）。
 * 行宽绝不超出容器：fitH（按容器宽反推）是硬顶，任何夹紧结果都不得宽于它（移动端窄屏遇全景图
 * 宽行时，0.5×下限/末行规则会把行推出视口）；非末行 0.5×–1.75× 夹紧（防过高/过矮），末行保持目标高。
 * 0 × 0 骨架（宽高未就绪）按 1:1 兜底。
 */
export function layoutRows(
  sk: readonly SkeletonLike[],
  containerWidth: number,
  targetH: number,
  opts: { gap?: number; metaH?: number; cardBorder?: number } = {},
): LayoutRow[] {
  const gap = opts.gap ?? GRID_GAP;
  const metaH = opts.metaH ?? CARD_META_H;
  const cardBorder = opts.cardBorder ?? CARD_BORDER;
  const width = containerWidth;
  const rows: LayoutRow[] = [];
  let y = 0;
  let row: { idx: number; id: string; ratio: number; star: number }[] = [];
  let ratiosSum = 0;

  const flush = (isLast: boolean) => {
    if (row.length === 0) {
      return;
    }
    const fitH = (width - (row.length - 1) * gap) / ratiosSum;
    const ideal = isLast ? targetH : Math.min(Math.max(fitH, targetH * 0.5), targetH * 1.75);
    const h = Math.max(1, Math.floor(Math.min(ideal, fitH)));
    // 行高 = 卡片总高（缩略图 + meta + 边框），行槽位与真实卡片一致，杜绝行间重叠
    const rowH = h + metaH + cardBorder;
    rows.push({
      key: row[0].id,
      cells: row.map((r) => ({ id: r.id, width: Math.round(h * r.ratio), height: h, star: r.star })),
      y,
      height: rowH,
      startIdx: row[0].idx,
      endIdx: row[row.length - 1].idx + 1,
    });
    y += rowH + gap;
    row = [];
    ratiosSum = 0;
  };

  for (let idx = 0; idx < sk.length; idx++) {
    const s = sk[idx];
    const ratio = s.width > 0 && s.height > 0 ? s.width / s.height : 1;
    if (row.length > 0 && (ratiosSum + ratio) * targetH + row.length * gap > width) {
      flush(false);
    }
    row.push({ idx, id: s.id, ratio, star: s.star });
    ratiosSum += ratio;
  }
  flush(true);
  return rows;
}

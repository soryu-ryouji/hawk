// 网格选中框的空间导航：ItemGrid 发布行布局（每项的视觉中心 x），useShortcuts 消费方向键。
import { ref } from 'vue';

export interface GridNavCell {
  id: string;
  /** 行内视觉中心 x（px） */
  cx: number;
}

/** 当前网格的行布局，由 ItemGrid 随布局变化更新 */
export const gridNavRows = ref<GridNavCell[][]>([]);

/** 方向键导航的一次性滚动标记：ItemGrid 的 primarySelected watcher 只在消费到此标记时才滚动视图。
 *  全选/鼠标点选/连选不应移动视口（Ctrl+A 语义：原地全选，滚动条不动） */
let pendingKeyboardScroll = false;

export function markKeyboardNavScroll(): void {
  pendingKeyboardScroll = true;
}

export function consumeKeyboardNavScroll(): boolean {
  const pending = pendingKeyboardScroll;
  pendingKeyboardScroll = false;
  return pending;
}

/**
 * 方向键移动选中：左右为线性前后（跨行连续），上下按视觉列中心对齐取最近项。
 * 无可达目标返回 null；无当前选中时返回第一项。
 */
export function moveGridSelection(
  rows: GridNavCell[][],
  currentId: string | null,
  dx: number,
  dy: number,
): string | null {
  if (rows.length === 0) {
    return null;
  }

  let rowIdx = -1;
  let colIdx = -1;
  for (let i = 0; i < rows.length && rowIdx < 0; i++) {
    const j = rows[i].findIndex((cell) => cell.id === currentId);
    if (j >= 0) {
      rowIdx = i;
      colIdx = j;
    }
  }
  if (rowIdx < 0) {
    return rows[0][0]?.id ?? null;
  }

  if (dx !== 0) {
    const flat = rows.flat();
    const idx = flat.findIndex((cell) => cell.id === currentId);
    return flat[idx + dx]?.id ?? null;
  }

  const targetRow = rows[rowIdx + dy];
  if (!targetRow || targetRow.length === 0) {
    return null;
  }
  const x = rows[rowIdx][colIdx].cx;
  let best = targetRow[0];
  for (const cell of targetRow) {
    if (Math.abs(cell.cx - x) < Math.abs(best.cx - x)) {
      best = cell;
    }
  }
  return best.id;
}

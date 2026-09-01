// layoutRows 齐行布局算法的单元测试：正常装行、fitH 硬顶（全景宽行）、末行规则、
// 行高夹紧上限、0×0 骨架兜底、空骨架。数学必须与 ItemGrid 虚拟渲染的槽位逐位一致。
import { describe, expect, it } from 'vitest';
import { CARD_BORDER, CARD_META_H, GRID_GAP, layoutRows, type SkeletonLike } from './layout';

const sk = (items: Array<[string, number, number]>): SkeletonLike[] =>
  items.map(([id, width, height]) => ({ id, width, height, star: 0 }));

/** 行内图片总宽 + 间隙（行宽占用校验用） */
const rowWidth = (row: ReturnType<typeof layoutRows>[number]) =>
  row.cells.reduce((sum, c) => sum + c.width, 0) + (row.cells.length - 1) * GRID_GAP;

describe('layoutRows', () => {
  it('空骨架返回空布局', () => {
    expect(layoutRows([], 800, 160)).toEqual([]);
  });

  it('单行单图：末行保持目标高，行高含 meta 与边框', () => {
    const rows = layoutRows(sk([['a', 400, 300]]), 800, 160);
    expect(rows).toHaveLength(1);
    expect(rows[0].cells[0]).toEqual({ id: 'a', width: 213, height: 160, star: 0 });
    expect(rows[0].height).toBe(160 + CARD_META_H + CARD_BORDER);
    expect(rows[0].startIdx).toBe(0);
    expect(rows[0].endIdx).toBe(1);
    expect(rows[0].y).toBe(0);
  });

  it('正常装行：累计超出容器宽即切行，非末行按容器宽反推', () => {
    // 4 张 400×300（ratio 4/3），容器 800、目标高 160：
    // 3 张累计 4×160 + 2×10 = 660 ≤ 800 同装一行；第 4 张 (4+4/3)×160 + 3×10 = 883 > 800 切行
    const rows = layoutRows(
      sk([
        ['a', 400, 300],
        ['b', 400, 300],
        ['c', 400, 300],
        ['d', 400, 300],
      ]),
      800,
      160,
    );
    expect(rows).toHaveLength(2);
    expect(rows[0].cells.map((c) => c.id)).toEqual(['a', 'b', 'c']);
    expect(rows[1].cells.map((c) => c.id)).toEqual(['d']);
    expect(rows[0].startIdx).toBe(0);
    expect(rows[0].endIdx).toBe(3);
    expect(rows[1].startIdx).toBe(3);
    expect(rows[1].endIdx).toBe(4);
    // 非末行 fitH = (800-20)/4 = 195（未触发夹紧）
    expect(rows[0].cells[0].height).toBe(195);
    expect(rows[0].cells[0].width).toBe(Math.round(195 * (4 / 3)));
    // 行宽 = 3×260 + 2×10 = 800，恰好填满容器
    expect(rowWidth(rows[0])).toBe(800);
    // 末行保持目标高
    expect(rows[1].cells[0].height).toBe(160);
    // 行 y 由上一行行高 + 行距推进
    expect(rows[1].y).toBe(rows[0].height + GRID_GAP);
  });

  it('行宽绝不超出容器（每行校验）', () => {
    const items: Array<[string, number, number]> = Array.from({ length: 30 }, (_, i) => [
      `i${i}`,
      200 + ((i * 137) % 800),
      200,
    ]);
    const rows = layoutRows(sk(items), 760, 160);
    expect(rows.length).toBeGreaterThan(1);
    for (const row of rows) {
      expect(rowWidth(row)).toBeLessThanOrEqual(760);
    }
  });

  it('全景图宽行 fitH 硬顶：末行单图也不得超出容器', () => {
    // 5000×1000 全景，容器 300：fitH = 300/5 = 60，远小于目标高 160
    const rows = layoutRows(sk([['pano', 5000, 1000]]), 300, 160);
    expect(rows).toHaveLength(1);
    expect(rows[0].cells[0].height).toBe(60);
    expect(rows[0].cells[0].width).toBe(300);
  });

  it('非末行夹紧上限 1.75×：fitH 过大时按上限行高', () => {
    // 10 张 18×100 窄条（ratio 0.18，R=1.8）+ 1 张 3000×1000 宽图（ratio 3）：
    // 宽图触发切行（(1.8+3)×160 + 10×10 = 868 > 800），窄条行非末行：
    // fitH = (800-90)/1.8 = 394 → 夹到 1.75×160 = 280
    const items: Array<[string, number, number]> = [...Array.from({ length: 10 }, (_, i) => [`s${i}`, 18, 100] as [string, number, number]), ['w', 3000, 1000]];
    const rows = layoutRows(sk(items), 800, 160);
    expect(rows).toHaveLength(2);
    expect(rows[0].cells).toHaveLength(10);
    expect(rows[0].cells[0].height).toBe(280);
    expect(rows[0].cells[0].width).toBe(Math.round(280 * 0.18));
    // 末行（宽图）保持目标高：fitH=266.7 但末行 ideal=160
    expect(rows[1].cells[0].height).toBe(160);
  });

  it('0×0 骨架按 1:1 兜底（宽高未就绪）', () => {
    const rows = layoutRows(sk([['z', 0, 0]]), 800, 160);
    expect(rows[0].cells[0].height).toBe(160);
    expect(rows[0].cells[0].width).toBe(160);
  });

  it('行高下限 1px（极端 fitH 不产出 0 高行）', () => {
    // 容器极窄：fitH 夹紧后仍为正，但 h 至少 1
    const rows = layoutRows(sk([['a', 1, 100]]), 5, 160);
    expect(rows[0].cells[0].height).toBeGreaterThanOrEqual(1);
  });

  it('star 透传到单元格（★ 角标）', () => {
    const rows = layoutRows([{ id: 'a', width: 400, height: 300, star: 3 }], 800, 160);
    expect(rows[0].cells[0].star).toBe(3);
  });
});

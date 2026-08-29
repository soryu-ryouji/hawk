// 素材拖拽（网格 → 侧栏文件夹/分类/标签）的共享工具。
// 拖拽源在 dragstart 写入 item id 列表；放置端在 dragover 期间只能读到 types（数据受保护），drop 时才可读数据。
export const ITEMS_MIME = 'application/x-hawk-items';

/** dragstart 侧：写入被拖拽的 item id 列表（调用方保证与当前选择语义一致） */
export function startItemsDrag(e: DragEvent, ids: string[]) {
  if (!e.dataTransfer) {
    return;
  }
  e.dataTransfer.setData(ITEMS_MIME, JSON.stringify(ids));
  e.dataTransfer.effectAllowed = 'move';
}

/** dragover 侧：仅当拖拽的是素材时放行（preventDefault 是允许 drop 的前提），否则返回 false。
 *  命中时必须 stopPropagation：useDropZone(document) 在 document 级也 cancel dragover/dragenter，
 *  按 DnD 处理模型冒泡后被 cancel 的【最外层】元素会成为 current target——drop 将落到 document 而非行上。 */
export function itemsDragOver(e: DragEvent): boolean {
  if (!e.dataTransfer?.types.includes(ITEMS_MIME)) {
    return false;
  }
  e.preventDefault();
  e.stopPropagation();
  e.dataTransfer.dropEffect = 'move';
  return true;
}

/** dragenter 侧：命中素材拖拽时同样阻断冒泡（document 级 dragenter cancel 同样会劫持 drop 目标） */
export function isItemsDrag(e: DragEvent): boolean {
  if (!e.dataTransfer?.types.includes(ITEMS_MIME)) {
    return false;
  }
  e.stopPropagation();
  return true;
}

/** drop 侧：取出 id 列表；非素材拖拽返回 null */
export function readItemsDrop(e: DragEvent): string[] | null {
  const raw = e.dataTransfer?.getData(ITEMS_MIME);
  if (!raw) {
    return null;
  }
  try {
    return JSON.parse(raw) as string[];
  } catch {
    return null;
  }
}

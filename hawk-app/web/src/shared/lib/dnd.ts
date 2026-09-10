// 拖拽共享工具：库内素材拖拽（网格 → 侧栏文件夹/分类/标签）与外部文件拖拽（Finder → 侧栏结构化导入）。
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

// ---- 外部文件拖拽（系统文件管理器/浏览器 → 窗口）：与素材拖拽同模式的判定/放行工具 ----

/** dragover 侧：仅当拖拽的是外部文件时放行（阻断冒泡的理由同 itemsDragOver） */
export function filesDragOver(e: DragEvent): boolean {
  if (!e.dataTransfer?.types.includes('Files')) {
    return false;
  }
  e.preventDefault();
  e.stopPropagation();
  e.dataTransfer.dropEffect = 'copy';
  return true;
}

/** dragenter 侧：外部文件拖拽命中时阻断冒泡（与 isItemsDrag 对称，防 document 级劫持 drop 目标） */
export function isFilesDrag(e: DragEvent): boolean {
  if (!e.dataTransfer?.types.includes('Files')) {
    return false;
  }
  e.stopPropagation();
  return true;
}

/** drop 侧：取出目录/文件 entry 列表（文件夹递归展开由 importer 的收集逻辑负责）；
 *  只能在 drop 处理器内同步调用（dataTransfer 生命周期所限），返回的 entry 可异步遍历 */
export function droppedEntries(e: DragEvent): FileSystemEntry[] {
  return [...(e.dataTransfer?.items ?? [])].map((item) => item.webkitGetAsEntry()).filter((entry): entry is FileSystemEntry => entry !== null);
}

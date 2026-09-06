// 拖拽保存（Eagle 式）：按住图片向某个方向拖过阈值后，面板浮在当前指针旁（留大间隙，避免被浏览器的拖拽幽灵图盖住）。
// 面板内容：
//   1. 「常用」文件夹：按保存次数自动统计的快捷投放入口；
//   2. 文件夹树：默认全折叠，拖拽悬停片刻自动展开，也可点击箭头展开/收起；投到某行存入对应文件夹；
//   3. 「＋ 新建文件夹」投放区：把图片拖到上面 → 命名 → 创建文件夹并把图片存入；平时点击也可只建文件夹。
// 投到左侧面板存入根目录；Esc 取消。
// content script 只负责交互与转发，实际保存/查询/新建经消息交给 background（content script 直连 hawk-daemon 会受 CORS 限制）。
import { browser } from 'wxt/browser';

const SAVE_MESSAGE = 'hawk:save-image';
const GET_FOLDERS_MESSAGE = 'hawk:get-folders';
const CREATE_FOLDER_MESSAGE = 'hawk:create-folder';
const NOTIFY_MESSAGE = 'hawk:notify';

/** 拖过多少像素后浮出保存面板（太小会在无意拖动时误触发） */
const DRAG_THRESHOLD = 60;
/** 面板边缘与指针的间隙：浏览器的拖拽幽灵图（半透明原图）以指针为中心、随原图大小变化，
 *  间隙必须足够大才不会被幽灵图盖住 */
const PANEL_GAP = 100;
/** 折叠的文件夹被拖拽悬停多久后自动展开（拖拽期间无法点击，只能靠悬停） */
const HOVER_EXPAND_DELAY = 700;

/** 常用文件夹条目（get-folders 消息返回） */
interface FlatNode {
  path: string;
  name: string;
}

/** 文件夹树节点（get-folders 消息返回） */
interface TreeNode {
  path: string;
  name: string;
  children: TreeNode[];
}

const FOLDER_ICON =
  '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>';
const CARET_ICON =
  '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<path d="m9 18 6-6-6-6"/></svg>';

let panel: HTMLDivElement | null = null;
let draggedSrc: string | null = null;
let startX = 0;
let startY = 0;
let dragging = false;
let cancelled = false;
/** 投到「新建文件夹」区块后待保存的图片（命名模式期间面板保留） */
let pendingSave: { url: string; pageUrl: string } | null = null;
/** 已展开的文件夹路径（目录默认全折叠，只记录展开项；同页面多次拖拽间保留） */
const expandedPaths = new Set<string>();
/** 当前面板展示的文件夹树（展开/收起时据此重绘） */
let currentTree: TreeNode | null = null;

export default defineContentScript({
  matches: ['http://*/*', 'https://*/*'],
  allFrames: true, // iframe 里的图片也能拖：面板出现在图片所在的文档内
  main() {
    injectStyles();
    document.addEventListener('dragstart', onDragStart, true);
    document.addEventListener('dragover', onDragOver, true); // HTML5 拖拽期间只有 dragover 能持续拿到指针位置
  },
});

function onDragStart(e: DragEvent) {
  const img = (e.target as HTMLElement | null)?.closest?.('img');
  const src = img?.currentSrc || img?.src;
  if (!src) {
    return; // 非图片拖拽（文本、链接等）不介入
  }
  cleanup();
  draggedSrc = src;
  startX = e.clientX;
  startY = e.clientY;
  dragging = true;
  window.addEventListener('dragend', cleanup, true); // 落在面板之外也要收起
  window.addEventListener('keydown', onKeyDown, true);
}

function onDragOver(e: DragEvent) {
  if (!dragging || panel || cancelled) {
    return;
  }
  const dx = e.clientX - startX;
  const dy = e.clientY - startY;
  if (Math.hypot(dx, dy) >= DRAG_THRESHOLD) {
    showPanel(dx, dy, e.clientX, e.clientY);
  }
}

/** 面板浮在指针旁：水平拖在左/右侧，垂直拖在上/下方（另一轴以指针为中心），边缘留 PANEL_GAP 间隙 */
function positionPanel(dx: number, dy: number, x: number, y: number, w: number, h: number): { left: string; top: string } {
  const MARGIN = 20;
  let left: number;
  let top: number;
  if (Math.abs(dx) >= Math.abs(dy)) {
    left = dx >= 0 ? x + PANEL_GAP : x - w - PANEL_GAP;
    top = y - h / 2;
  } else {
    left = x - w / 2;
    top = dy >= 0 ? y + PANEL_GAP : y - h - PANEL_GAP;
  }
  // 视口钳制（左/上最小 20，底部留 20，右侧留 10）
  left = Math.min(Math.max(left, MARGIN), window.innerWidth - w - 10);
  top = Math.min(Math.max(top, MARGIN), window.innerHeight - h - MARGIN);
  return { left: `${left}px`, top: `${top}px` };
}

function showPanel(dx: number, dy: number, x: number, y: number) {
  panel = document.createElement('div');
  panel.className = 'hawk-drop-panel';
  panel.style.visibility = 'hidden'; // 先渲染量尺寸，定位后再显示，避免闪烁
  panel.innerHTML = `
    <div class="hawk-drop-zone" title="保存到根目录">
      <svg width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
      </svg>
      <span class="hawk-drop-zone-title">保存到 hawk</span>
      <small>根目录</small>
    </div>
    <div class="hawk-drop-lists">
      <div class="hawk-drop-frequent" hidden></div>
      <div class="hawk-drop-rows">
        <div class="hawk-drop-hint">加载中…</div>
      </div>
      <div class="hawk-drop-create" title="拖入图片：新建文件夹并保存；点击：仅新建文件夹">
        <span class="hawk-drop-create-label">＋ 新建文件夹</span>
        <small>拖入图片即建文件夹并保存</small>
      </div>
    </div>`;
  makeDroppable(panel.querySelector('.hawk-drop-zone')!, { folderPath: '' }); // 空串 = 存根目录
  const createBlock = panel.querySelector<HTMLElement>('.hawk-drop-create')!;
  makeDroppable(createBlock, {}); // folderPath 缺省 = 进入命名模式（新建文件夹并保存）
  createBlock.addEventListener('click', () => {
    if (!dragging) {
      enterNamingMode(); // 非拖拽时点击 = 只建文件夹
    }
  });
  document.documentElement.appendChild(panel);

  // 量出实际尺寸后按指针位置定位，再显示
  const pos = positionPanel(dx, dy, x, y, panel.offsetWidth, panel.offsetHeight);
  panel.style.left = pos.left;
  panel.style.top = pos.top;
  panel.style.visibility = '';

  void loadFolders();
}

async function loadFolders() {
  const rows = panel?.querySelector<HTMLElement>('.hawk-drop-rows');
  if (!panel || !rows) {
    return;
  }
  let data: { folders: TreeNode; frequent: FlatNode[] };
  try {
    data = (await browser.runtime.sendMessage({ type: GET_FOLDERS_MESSAGE })) as { folders: TreeNode; frequent: FlatNode[] };
  } catch {
    rows.innerHTML = '<div class="hawk-drop-hint">加载失败</div>';
    return;
  }
  if (!panel) {
    return; // 等待响应期间面板可能已被关闭
  }
  const tree = data && Array.isArray(data.folders?.children) ? data.folders : { path: '', name: '', children: [] };
  const frequent = data && Array.isArray(data.frequent) ? data.frequent : [];
  renderFolders(tree, frequent);
}

/** 渲染「常用」区与文件夹树 */
function renderFolders(tree: TreeNode, frequent: FlatNode[]) {
  if (!panel) {
    return;
  }
  currentTree = tree;
  renderFrequent(panel.querySelector<HTMLElement>('.hawk-drop-frequent')!, frequent);
  const rows = panel.querySelector<HTMLElement>('.hawk-drop-rows')!;
  rows.innerHTML = '';
  if (tree.children.length === 0) {
    rows.innerHTML = '<div class="hawk-drop-hint">（暂无文件夹）</div>';
    return;
  }
  renderTreeRows(rows, tree.children, 0);
}

/** 「常用」：按保存次数排序的快捷投放 chips，可拖入图片 */
function renderFrequent(container: HTMLElement, frequent: FlatNode[]) {
  container.innerHTML = '';
  container.hidden = frequent.length === 0;
  if (frequent.length === 0) {
    return;
  }
  const title = document.createElement('div');
  title.className = 'hawk-drop-section-title';
  title.textContent = '常用';
  const chips = document.createElement('div');
  chips.className = 'hawk-drop-chips';
  for (const folder of frequent) {
    const chip = document.createElement('div');
    chip.className = 'hawk-drop-chip';
    chip.title = folder.path;
    chip.innerHTML = `${FOLDER_ICON}<span>${escapeHtml(folder.name)}</span>`;
    makeDroppable(chip, { folderPath: folder.path });
    chips.appendChild(chip);
  }
  container.append(title, chips);
}

/** 递归渲染树行；只渲染已展开的分支（默认全折叠） */
function renderTreeRows(container: HTMLElement, nodes: TreeNode[], depth: number) {
  for (const node of nodes) {
    container.appendChild(makeTreeRow(node, depth));
    if (expandedPaths.has(node.path) && node.children.length > 0) {
      renderTreeRows(container, node.children, depth + 1);
    }
  }
}

function makeTreeRow(node: TreeNode, depth: number): HTMLElement {
  const row = document.createElement('div');
  row.className = 'hawk-drop-row';
  row.title = node.path;
  row.style.paddingLeft = `${12 + depth * 16}px`;
  if (node.children.length > 0) {
    const caret = document.createElement('span');
    caret.className = 'hawk-drop-caret';
    caret.title = expandedPaths.has(node.path) ? '收起' : '展开';
    caret.innerHTML = CARET_ICON;
    if (expandedPaths.has(node.path)) {
      row.classList.add('hawk-open');
    }
    caret.addEventListener('click', (e) => {
      e.stopPropagation();
      if (!panel) {
        return;
      }
      if (expandedPaths.has(node.path)) {
        expandedPaths.delete(node.path);
      } else {
        expandedPaths.add(node.path);
      }
      rerenderTree();
    });
    row.appendChild(caret);
    armHoverExpand(row, node); // 拖拽中无法点击，靠悬停自动展开
  } else {
    const spacer = document.createElement('span');
    spacer.className = 'hawk-drop-caret-spacer';
    row.appendChild(spacer);
  }
  row.insertAdjacentHTML('beforeend', `${FOLDER_ICON}<span class="hawk-drop-row-name">${escapeHtml(node.name)}</span>`);
  makeDroppable(row, { folderPath: node.path });
  return row;
}

/** 按当前展开状态重绘文件夹树（保持滚动位置） */
function rerenderTree() {
  const rows = panel?.querySelector<HTMLElement>('.hawk-drop-rows');
  if (!rows || !currentTree) {
    return;
  }
  const scrollTop = rows.scrollTop;
  rows.innerHTML = '';
  renderTreeRows(rows, currentTree.children, 0);
  rows.scrollTop = scrollTop;
}

/** 折叠的文件夹被拖拽悬停一段时间后自动展开 */
function armHoverExpand(row: HTMLElement, node: TreeNode) {
  let timer: number | undefined;
  const clear = () => {
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
    }
  };
  row.addEventListener('dragover', () => {
    if (timer === undefined && !expandedPaths.has(node.path)) {
      timer = window.setTimeout(() => {
        timer = undefined;
        if (panel && !expandedPaths.has(node.path)) {
          expandedPaths.add(node.path);
          rerenderTree();
        }
      }, HOVER_EXPAND_DELAY);
    }
  });
  row.addEventListener('dragleave', clear);
  row.addEventListener('drop', clear);
}

function escapeHtml(text: string): string {
  return text.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]!);
}

/** 新建文件夹：区块切换为内联输入。pendingSave 存在时（拖入图片触发），命名后连图片一起入库 */
function enterNamingMode() {
  const block = panel?.querySelector<HTMLElement>('.hawk-drop-create');
  if (!panel || !block || block.querySelector('input')) {
    return;
  }
  block.classList.add('hawk-drop-naming');
  block.innerHTML = '';
  const input = document.createElement('input');
  input.className = 'hawk-drop-create-input';
  input.placeholder = pendingSave ? '新文件夹名称（存入该文件夹）' : '文件夹名称';
  block.appendChild(input);
  input.focus();

  input.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.stopPropagation(); // 面板级的 Esc 取消统一走这里
      cancelNaming();
      return;
    }
    if (e.key === 'Enter') {
      e.stopPropagation();
      void submitNaming(input.value.trim());
    }
  });
}

async function submitNaming(name: string) {
  const pending = pendingSave;
  cancelNaming();
  if (!name) {
    return;
  }
  try {
    await browser.runtime.sendMessage({ type: CREATE_FOLDER_MESSAGE, value: name });
    if (pending) {
      await browser.runtime.sendMessage({ type: SAVE_MESSAGE, url: pending.url, pageUrl: pending.pageUrl, folderPath: name });
    } else {
      // 纯新建（点击触发）：刷新列表让新文件夹立即可投放
      await loadFolders();
    }
  } catch (e) {
    void browser.runtime.sendMessage({
      type: NOTIFY_MESSAGE,
      message: `创建文件夹失败：${e instanceof Error ? e.message : String(e)}`,
    });
  }
}

/** 取消命名：收起面板并丢弃待保存图片 */
function cancelNaming() {
  pendingSave = null;
  cleanup();
}

/** 让元素成为投放目标；extra.folderPath 非空时存入该文件夹，否则存根目录 */
function makeDroppable(el: HTMLElement, extra: { folderPath?: string }) {
  el.addEventListener('dragover', (e) => {
    e.preventDefault(); // 必须阻止默认，否则无法触发 drop
    e.stopPropagation();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect = 'copy';
    }
    el.classList.add('hawk-drop-over');
  });
  el.addEventListener('dragleave', () => el.classList.remove('hawk-drop-over'));
  el.addEventListener('drop', (e) => {
    e.preventDefault();
    e.stopPropagation();
    const src = draggedSrc;
    const pageUrl = location.href;
    const folderPath = extra.folderPath;
    if (!src || cancelled) {
      cleanup();
      return;
    }
    if (folderPath !== undefined) {
      // 投到文件夹行 / 根目录区域：直接保存
      cleanup();
      void browser.runtime.sendMessage({ type: SAVE_MESSAGE, url: src, pageUrl, ...(folderPath ? { folderPath } : {}) });
      return;
    }
    // 投到「新建文件夹」区块：进入命名模式，面板保留等待输入
    pendingSave = { url: src, pageUrl };
    draggedSrc = null;
    enterNamingMode();
  });
}

function cleanup() {
  if (pendingSave) {
    // 命名模式：dragend 会走到这里，但面板要保留等输入，只解除拖拽态
    dragging = false;
    draggedSrc = null;
    window.removeEventListener('dragend', cleanup, true);
    return;
  }
  if (!dragging && !panel) {
    return;
  }
  dragging = false;
  draggedSrc = null;
  cancelled = false;
  panel?.remove();
  panel = null;
  window.removeEventListener('dragend', cleanup, true);
  window.removeEventListener('keydown', onKeyDown, true);
}

/** Esc 取消本次拖拽保存（HTML5 拖拽无法编程取消，只能收起面板并忽略 drop） */
function onKeyDown(e: KeyboardEvent) {
  if (e.key === 'Escape') {
    cancelled = true;
    cancelNaming();
  }
}

function injectStyles() {
  const style = document.createElement('style');
  style.textContent = `
.hawk-drop-panel {
  position: fixed;
  z-index: 2147483647;
  display: flex;
  gap: 12px;
  padding: 14px;
  border-radius: 14px;
  border: 1px solid #2c313a;
  background: rgba(22, 24, 29, 0.96);
  box-shadow: 0 12px 44px rgba(0, 0, 0, 0.55);
  color: #d6d9de;
  font: 14px/1.5 system-ui, 'Segoe UI', 'Microsoft YaHei', sans-serif;
}
/* 左：大投放区（Eagle 式），随右栏高度撑开 */
.hawk-drop-zone {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  width: 220px;
  border: 1.5px dashed #3a4150;
  border-radius: 10px;
  color: #8a8f98;
  text-align: center;
}
.hawk-drop-zone-title {
  font-size: 15px;
  color: #d6d9de;
}
.hawk-drop-zone small {
  font-size: 12px;
}
/* 右：常用 + 文件夹树 + 新建区块 */
.hawk-drop-lists {
  display: flex;
  flex-direction: column;
  width: 250px;
  /* 比原先更高，减少滚动；iframe 内以视口为上限 */
  max-height: min(440px, calc(100vh - 80px));
}
.hawk-drop-rows {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 2px;
  overflow-y: auto;
  padding-right: 2px;
  /* 细滚动条：Firefox 走 scrollbar-*，Chromium/Safari 走 ::-webkit-scrollbar */
  scrollbar-width: thin;
  scrollbar-color: #3a4150 transparent;
}
.hawk-drop-rows::-webkit-scrollbar {
  width: 8px;
}
.hawk-drop-rows::-webkit-scrollbar-track {
  background: transparent;
}
.hawk-drop-rows::-webkit-scrollbar-thumb {
  border: 2px solid transparent;
  border-radius: 999px;
  background: #3a4150;
  background-clip: padding-box;
}
.hawk-drop-rows::-webkit-scrollbar-thumb:hover {
  background-color: #4d5563;
}
/* 「常用」：按保存次数排序的快捷投放入口 */
.hawk-drop-frequent {
  flex: none;
  margin-bottom: 8px;
  padding-bottom: 10px;
  border-bottom: 1px solid #2c313a;
}
.hawk-drop-section-title {
  margin-bottom: 6px;
  color: #6b7078;
  font-size: 12px;
}
.hawk-drop-chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.hawk-drop-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  max-width: 100%;
  padding: 4px 10px;
  border-radius: 999px;
  background: #23272f;
  color: #b9bdc4;
  font-size: 12.5px;
  cursor: default;
}
.hawk-drop-chip svg {
  flex: none;
  color: #8a8f98;
}
.hawk-drop-chip span {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.hawk-drop-chip:hover {
  background: #2a2f39;
}
.hawk-drop-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 10px;
  border-radius: 8px;
  font-size: 14px;
  white-space: nowrap;
  overflow: hidden;
  cursor: default;
}
/* 展开/收起箭头（无子级的行用同宽占位符对齐） */
.hawk-drop-caret {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
  width: 16px;
  height: 16px;
  border-radius: 4px;
  color: #8a8f98;
  cursor: pointer;
  transition: transform 0.12s ease;
}
.hawk-drop-caret:hover {
  background: #2a2f39;
}
.hawk-drop-caret:hover svg {
  color: #d6d9de;
}
.hawk-drop-row.hawk-open .hawk-drop-caret {
  transform: rotate(90deg);
}
.hawk-drop-caret-spacer {
  flex: none;
  width: 16px;
}
.hawk-drop-row svg {
  flex: none;
  color: #8a8f98;
}
.hawk-drop-row:hover {
  background: #23272f;
}
.hawk-drop-row-name {
  overflow: hidden;
  text-overflow: ellipsis;
}
/* 「新建文件夹」投放区：分隔线 + 整行（Eagle 式），可投放也可点击 */
.hawk-drop-create {
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin-top: 8px;
  padding: 12px;
  border-top: 1px solid #2c313a;
  color: #4a7dff;
  text-align: center;
  cursor: pointer;
}
.hawk-drop-create-label {
  font-size: 14px;
}
.hawk-drop-create small {
  font-size: 12px;
  color: #6b7078;
}
.hawk-drop-create:hover {
  color: #5d8bff;
}
.hawk-drop-naming {
  cursor: default;
}
.hawk-drop-create-input {
  box-sizing: border-box;
  width: 100%;
  padding: 8px 10px;
  border: 1px solid #4a7dff;
  border-radius: 6px;
  background: #1d2026;
  color: #d6d9de;
  font: 14px/1.4 system-ui, 'Segoe UI', 'Microsoft YaHei', sans-serif;
}
.hawk-drop-create-input:focus {
  outline: none;
}
.hawk-drop-hint {
  padding: 10px 12px;
  color: #6b7078;
  font-size: 13px;
}
.hawk-drop-over {
  border-color: #4a7dff !important;
  background: rgba(74, 125, 255, 0.2) !important;
  color: #fff !important;
}
.hawk-drop-over svg {
  color: #fff !important;
}`;
  document.documentElement.appendChild(style);
}

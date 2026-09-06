// 拖拽保存（Eagle 式）：按住图片拖过阈值后，面板锚定在当前指针处向右下展开，左侧大投放区正好压在指针下——
// 面板出现即可松手存入根目录；右移可投入具体文件夹（按最近使用排序）。
// 面板内容：
//   1. 文件夹列表：只显示根目录下的直接子文件夹，不展开层级——拖拽期间 click 被浏览器抑制，
//      展开项无法收起，干脆不做层级；
//   2. 「＋ 新建文件夹」投放区：把图片拖到上面 → 命名 → 创建文件夹并把图片存入；平时点击也可只建文件夹。
// 投到左侧面板存入根目录；Esc 取消。
// 原生拖拽残影是大图的不透明拷贝，会盖住面板文字 → dragstart 时用 setDragImage 换成小尺寸、半透明轻虚化的缩略图。
// content script 只负责交互与转发，实际保存/查询/新建经消息交给 background（content script 直连 hawk-daemon 会受 CORS 限制）。
import { browser } from 'wxt/browser';

const SAVE_MESSAGE = 'hawk:save-image';
const GET_FOLDERS_MESSAGE = 'hawk:get-folders';
const CREATE_FOLDER_MESSAGE = 'hawk:create-folder';
const NOTIFY_MESSAGE = 'hawk:notify';

/** 拖过多少像素后浮出保存面板：面板出现后指针就落在保存区上，误触发会直接入库，阈值要比一般拖拽更大 */
const DRAG_THRESHOLD = 80;
/** Eagle 式锚定：面板出现时指针落在左侧大投放区内（松手即保存到根目录）。
 *  锚定偏移与 .hawk-drop-panel 的内边距 / .hawk-drop-zone 的宽度保持一致 */
const ANCHOR_PAD = 14;
const ZONE_WIDTH = 220;
const ANCHOR_INSET_X = 40; // 指针距区块左内缘
const ANCHOR_INSET_Y = 12; // 指针距区块顶内缘
/** 拖拽残影缩略图的最大宽高（px）；与 .hawk-drag-thumb img 的 max-width/max-height 保持一致 */
const DRAG_THUMB_MAX = 120;

/** 文件夹树节点（get-folders 消息返回，已按最近使用排序） */
interface TreeNode {
  path: string;
  name: string;
  children: TreeNode[];
}

const FOLDER_ICON =
  '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>';

let panel: HTMLDivElement | null = null;
let draggedSrc: string | null = null;
/** setDragImage 的快照源元素（离屏渲染的小缩略图）；浏览器在 dragstart 之后才截图，须保留到拖拽结束 */
let dragThumb: HTMLDivElement | null = null;
let startX = 0;
let startY = 0;
let dragging = false;
let cancelled = false;
/** 投到「新建文件夹」区块后待保存的图片（命名模式期间面板保留） */
let pendingSave: { url: string; pageUrl: string } | null = null;

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
  setDragThumbnail(e, src);
  startX = e.clientX;
  startY = e.clientY;
  dragging = true;
  window.addEventListener('dragend', cleanup, true); // 落在面板之外也要收起
  window.addEventListener('keydown', onKeyDown, true);
}

/** 用离屏小缩略图替换原生拖拽残影：元素须真实渲染（不能 display:none/opacity:0），
 *  固定在视口外即可；setDragImage 必须在 dragstart 处理器内同步调用，元素保留到 cleanup 再移除 */
function setDragThumbnail(e: DragEvent, src: string) {
  if (!e.dataTransfer) {
    return;
  }
  removeDragThumbnail();
  const thumb = document.createElement('div');
  thumb.className = 'hawk-drag-thumb';
  const img = document.createElement('img');
  img.src = src;
  thumb.appendChild(img);
  document.documentElement.appendChild(thumb);
  // 读 offsetWidth 强制同步布局，拿到缩略后的实际尺寸；图片未就绪时兜底为中心偏移
  const w = img.offsetWidth || DRAG_THUMB_MAX;
  const h = img.offsetHeight || DRAG_THUMB_MAX;
  e.dataTransfer.setDragImage(thumb, Math.round(w / 2), Math.round(h / 2)); // 指针落在缩略图中心
  dragThumb = thumb;
}

function removeDragThumbnail() {
  dragThumb?.remove();
  dragThumb = null;
}

function onDragOver(e: DragEvent) {
  if (!dragging || panel || cancelled) {
    return;
  }
  const dx = e.clientX - startX;
  const dy = e.clientY - startY;
  if (Math.hypot(dx, dy) >= DRAG_THRESHOLD) {
    showPanel(e.clientX, e.clientY);
  }
}

/** Eagle 式锚定：面板自指针处向右下展开，让指针落进左侧大投放区（距区块左内缘 ANCHOR_INSET_X、
 *  顶内缘 ANCHOR_INSET_Y）——出现即可松手保存到根目录；贴近视口边缘时逐轴翻转，指针始终保持在区块内 */
function positionPanel(x: number, y: number, w: number, h: number): { left: string; top: string } {
  const MARGIN = 8;
  let left = x - ANCHOR_PAD - ANCHOR_INSET_X;
  let top = y - ANCHOR_PAD - ANCHOR_INSET_Y;
  left = Math.min(Math.max(left, MARGIN), window.innerWidth - w - MARGIN);
  // 近右缘被钳制后指针滑到区块右侧 → 翻到指针左侧展开（指针改落在区块右内缘处）
  if (x > left + ANCHOR_PAD + ZONE_WIDTH) {
    left = Math.max(x - ANCHOR_PAD - ZONE_WIDTH + ANCHOR_INSET_X, MARGIN);
  }
  top = Math.min(Math.max(top, MARGIN), window.innerHeight - h - MARGIN);
  // 近下缘被钳制后指针落到面板之外 → 翻到指针上方展开（指针改落在区块底内缘处）
  if (y > top + h) {
    top = Math.max(y + ANCHOR_PAD + ANCHOR_INSET_Y - h, MARGIN);
  }
  return { left: `${left}px`, top: `${top}px` };
}

function showPanel(x: number, y: number) {
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

  // 量出实际尺寸后锚定到指针处，再显示
  const pos = positionPanel(x, y, panel.offsetWidth, panel.offsetHeight);
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
  let data: { folders: TreeNode };
  try {
    data = (await browser.runtime.sendMessage({ type: GET_FOLDERS_MESSAGE })) as { folders: TreeNode };
  } catch {
    rows.innerHTML = '<div class="hawk-drop-hint">加载失败</div>';
    return;
  }
  if (!panel) {
    return; // 等待响应期间面板可能已被关闭
  }
  const tree = data && Array.isArray(data.folders?.children) ? data.folders : { path: '', name: '', children: [] };
  renderFolders(tree);
}

/** 渲染文件夹列表：只显示根目录下的直接子文件夹（不做层级展开，见文件头注释；已按最近使用排序） */
function renderFolders(tree: TreeNode) {
  if (!panel) {
    return;
  }
  const rows = panel.querySelector<HTMLElement>('.hawk-drop-rows')!;
  rows.innerHTML = '';
  if (tree.children.length === 0) {
    rows.innerHTML = '<div class="hawk-drop-hint">（暂无文件夹）</div>';
    return;
  }
  for (const node of tree.children) {
    rows.appendChild(makeTreeRow(node));
  }
}

function makeTreeRow(node: TreeNode): HTMLElement {
  const row = document.createElement('div');
  row.className = 'hawk-drop-row';
  row.title = node.path;
  row.insertAdjacentHTML('beforeend', `${FOLDER_ICON}<span class="hawk-drop-row-name">${escapeHtml(node.name)}</span>`);
  makeDroppable(row, { folderPath: node.path });
  return row;
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
  removeDragThumbnail(); // 拖拽已结束，残影快照源一并移除（命名模式分支同样适用）
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
/* 拖拽残影的快照源：离屏渲染的小缩略图；max 尺寸须与 DRAG_THUMB_MAX 一致。
   opacity / filter 会烘焙进快照 —— 这正是残影半透明 + 轻虚化的实现方式；
   但不能用来隐身（display:none、opacity:0 快照变空，transform 会位移快照），只能移出视口 */
.hawk-drag-thumb {
  position: fixed;
  top: -10000px;
  left: -10000px;
  opacity: 0.1;
  filter: blur(1.5px);
  border-radius: 8px;
  overflow: hidden;
  border: 1px solid rgba(255, 255, 255, 0.75);
  background: rgba(22, 24, 29, 0.9);
}
.hawk-drag-thumb img {
  display: block;
  max-width: 120px;
  max-height: 120px;
}
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
/* 右：文件夹树 + 新建区块 */
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

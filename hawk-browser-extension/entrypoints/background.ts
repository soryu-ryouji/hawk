// 后台：右键菜单「保存图片到 hawk」入口，负责与 hawk-daemon 通信并反馈结果。
// MV3 下 contextMenus 须在 onInstalled 里创建，避免 service worker 重启后重复注册。
import { browser } from 'wxt/browser';
import { addItemByBase64, addItemByUrl, createFolder, fetchFolderList, type FolderNode } from '../lib/api';
import { notify } from '../lib/notify';

const MENU_ID = 'hawk-save-image';
const SAVE_MESSAGE = 'hawk:save-image';
const GET_FOLDERS_MESSAGE = 'hawk:get-folders';
const CREATE_FOLDER_MESSAGE = 'hawk:create-folder';
const NOTIFY_MESSAGE = 'hawk:notify';

/** 常用文件夹条目（拖拽保存面板展示用） */
export interface FlatNode {
  path: string;
  name: string;
}

/** 各文件夹累计保存次数（storage.local 持久化，用于「常用」列表） */
type FolderUsage = Record<string, number>;
const USAGE_KEY = 'folderUsage';
/** 「常用」列表条数上限 */
const FREQUENT_LIMIT = 5;

let foldersCache: { folders: FolderNode; frequent: FlatNode[]; at: number } | null = null;
const FOLDERS_CACHE_TTL = 30_000;

function flattenTree(root: { path: string; name: string; children: { path: string; name: string; children: unknown[] }[] }): FlatNode[] {
  const list: FlatNode[] = [];
  const walk = (children: typeof root.children) => {
    for (const child of children) {
      list.push({ path: child.path, name: child.name });
      walk(child.children as typeof root.children);
    }
  };
  walk(root.children);
  return list;
}

async function getUsage(): Promise<FolderUsage> {
  const stored = await browser.storage.local.get(USAGE_KEY);
  return (stored[USAGE_KEY] as FolderUsage | undefined) ?? {};
}

/** 保存成功后累计次数，并作废缓存让下次「常用」列表重算 */
async function recordUsage(path: string) {
  const usage = await getUsage();
  usage[path] = (usage[path] ?? 0) + 1;
  await browser.storage.local.set({ [USAGE_KEY]: usage });
  foldersCache = null;
}

/** 按保存次数取前几个仍存在于库中的文件夹 */
function computeFrequent(folders: FlatNode[], usage: FolderUsage): FlatNode[] {
  const byPath = new Map(folders.map((f) => [f.path, f]));
  return Object.entries(usage)
    .filter(([path, count]) => count > 0 && byPath.has(path))
    .sort((a, b) => b[1] - a[1])
    .slice(0, FREQUENT_LIMIT)
    .map(([path]) => byPath.get(path)!);
}

/** 文件夹树 + 常用文件夹（拖拽保存面板展示用） */
async function getFolders(force = false): Promise<{ folders: FolderNode; frequent: FlatNode[] }> {
  if (!force && foldersCache && Date.now() - foldersCache.at < FOLDERS_CACHE_TTL) {
    const { folders, frequent } = foldersCache;
    return { folders, frequent };
  }
  const folders = await fetchFolderList();
  const frequent = computeFrequent(flattenTree(folders), await getUsage());
  foldersCache = { folders, frequent, at: Date.now() };
  return { folders, frequent };
}

export default defineBackground(() => {
  browser.runtime.onInstalled.addListener(() => {
    browser.contextMenus.create({
      id: MENU_ID,
      title: '保存图片到 hawk',
      contexts: ['image'],
    });
  });

  browser.contextMenus.onClicked.addListener((info, tab) => {
    if (info.menuItemId !== MENU_ID || !info.srcUrl) {
      return;
    }
    void saveImage(info.srcUrl, tab?.url);
  });

  // 拖拽保存：content script 经消息转发图片地址 / 索取与新建文件夹
  browser.runtime.onMessage.addListener((message: unknown) => {
    if (isSaveMessage(message)) {
      void saveImage(message.url, message.pageUrl, message.folderPath);
      return;
    }
    if (isMessageOfType(message, GET_FOLDERS_MESSAGE)) {
      return getFolders();
    }
    if (isCreateMessage(message, CREATE_FOLDER_MESSAGE)) {
      return createFolder(message.value).then(() => getFolders(true));
    }
    if (isNotifyMessage(message)) {
      void notify(message.message); // content script 无 notifications 权限，代为弹出
      return;
    }
  });
});

function messageType(message: unknown): unknown {
  return typeof message === 'object' && message !== null ? (message as { type?: unknown }).type : undefined;
}

function isMessageOfType(message: unknown, type: string): boolean {
  return messageType(message) === type;
}

function isSaveMessage(message: unknown): message is { type: string; url: string; pageUrl?: string; folderPath?: string } {
  return messageType(message) === SAVE_MESSAGE && typeof (message as { url?: unknown }).url === 'string';
}

function isCreateMessage(message: unknown, type: string): message is { type: string; value: string } {
  return messageType(message) === type && typeof (message as { value?: unknown }).value === 'string';
}

function isNotifyMessage(message: unknown): message is { type: string; message: string } {
  return messageType(message) === NOTIFY_MESSAGE && typeof (message as { message?: unknown }).message === 'string';
}

async function saveImage(srcUrl: string, pageUrl?: string, folderPath?: string) {
  try {
    if (srcUrl.startsWith('data:image/')) {
      // data URL 直接转 base64 提交，无需下载
      await addItemByBase64(srcUrl.slice(srcUrl.indexOf(',') + 1), pageUrl, folderPath);
    } else if (/^https?:\/\//.test(srcUrl)) {
      await addItemByUrl(srcUrl, pageUrl, folderPath);
    } else {
      throw new Error('不支持的图片地址（blob: 需要页面脚本协助，暂未支持）');
    }
    if (folderPath) {
      void recordUsage(folderPath).catch(() => {}); // 计入常用统计，失败不影响保存结果
    }
    await notify('已保存到 hawk');
  } catch (e) {
    await notify(`保存失败：${e instanceof Error ? e.message : String(e)}`);
  }
}

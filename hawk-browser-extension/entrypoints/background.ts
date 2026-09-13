// 后台：右键菜单「保存图片到 hawk」入口，负责与 hawk-daemon 通信并反馈结果。
// MV3 下 contextMenus 须在 onInstalled 里创建，避免 service worker 重启后重复注册。
import { browser } from 'wxt/browser';
import { addItemByBase64, createFolder, fetchFolderList, HttpError, type FolderNode } from '../lib/api';
import { notify } from '../lib/notify';

const MENU_ID = 'hawk-save-image';
const SAVE_MESSAGE = 'hawk:save-image';
const GET_FOLDERS_MESSAGE = 'hawk:get-folders';
const CREATE_FOLDER_MESSAGE = 'hawk:create-folder';
const NOTIFY_MESSAGE = 'hawk:notify';

/** 各文件夹最近一次保存的时间戳（storage.local 持久化，用于目录按最近使用排序） */
type FolderUsage = Record<string, number>;
const USAGE_KEY = 'folderLastUse';

let foldersCache: { folders: FolderNode; at: number } | null = null;
const FOLDERS_CACHE_TTL = 30_000;

async function getUsage(): Promise<FolderUsage> {
  const stored = await browser.storage.local.get(USAGE_KEY);
  return (stored[USAGE_KEY] as FolderUsage | undefined) ?? {};
}

/** 保存成功后记录时间戳，并作废缓存让下次面板拿到新排序 */
async function recordUsage(path: string) {
  const usage = await getUsage();
  usage[path] = Date.now();
  await browser.storage.local.set({ [USAGE_KEY]: usage });
  foldersCache = null;
}

/** 目录按最近使用排序：用过的在前（最近优先），没用过的保持服务端顺序排在后面；每层都排 */
function sortByLastUse(nodes: FolderNode[], usage: FolderUsage): FolderNode[] {
  return nodes
    .map((node) => ({ ...node, children: sortByLastUse(node.children, usage) }))
    .sort((a, b) => (usage[b.path] ?? 0) - (usage[a.path] ?? 0));
}

/** 按最近使用排序后的文件夹树（拖拽保存面板展示用） */
async function getFolders(force = false): Promise<{ folders: FolderNode }> {
  if (!force && foldersCache && Date.now() - foldersCache.at < FOLDERS_CACHE_TTL) {
    return { folders: foldersCache.folders };
  }
  const root = await fetchFolderList();
  const folders = { ...root, children: sortByLastUse(root.children, await getUsage()) };
  foldersCache = { folders, at: Date.now() };
  return { folders };
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

  // Firefox(MV2) 无 DNR：webRequest blocking 改写防盗链站点的 Referer
  //（URL filter 限定站点表域名，非匹配请求零开销）；Chrome/Safari 走按需注册的 DNR
  if (
    typeof browser.declarativeNetRequest?.updateSessionRules !== 'function' &&
    browser.webRequest?.onBeforeSendHeaders &&
    !browser.webRequest.onBeforeSendHeaders.hasListener(rewriteRefererFirefox)
  ) {
    browser.webRequest.onBeforeSendHeaders.addListener(
      rewriteRefererFirefox,
      { urls: REFERER_SITES.flatMap((site) => site.domains.map((d) => `*://*.${d}/*`)) },
      ['blocking', 'requestHeaders'],
    );
  }

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
      // 浏览器网络栈下载（真实 Chrome TLS 指纹 + 用户代理与会话），转 base64 提交。
      // 服务端下载已移除：ureq 无代理无会话、TLS 指纹常被目标站拒连，
      // 能力是浏览器网络栈的子集，回退兑底没有实际价值
      await addItemByBase64(await downloadAsBase64(srcUrl), pageUrl, folderPath);
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

// ---------- 防盗链 Referer 改写 ----------
// 部分图床校验 Referer（如 pixiv 的 i.pximg.net：非 pixiv 来源一律 403），而
// service worker 的 fetch 无法设置跨源 Referer（fetch 规范禁止），只能经请求头
// 改写 API 补上。遇到新防盗链站点往表里加一行即可。
const REFERER_SITES: { domains: string[]; referer: string }[] = [
  { domains: ['pximg.net'], referer: 'https://www.pixiv.net/' },
];

/** url 命中站点表时返回对应改写规则 */
function refererRewriteFor(url: string): { domains: string[]; referer: string } | undefined {
  try {
    const host = new URL(url).hostname;
    return REFERER_SITES.find((site) => site.domains.some((d) => host === d || host.endsWith(`.${d}`)));
  } catch {
    return undefined;
  }
}

const DNR_RULE_BASE = 1000; // 会话规则 id 基址，避开其他来源的规则
let refererRulesInstalled: Promise<void> | null = null;

/** Chrome/Safari：注册 Referer 改写的 DNR 会话规则（幂等；service worker 重启后规则丢失，会重新注册） */
function installRefererRulesDnr(): Promise<void> {
  const rules: Browser.declarativeNetRequest.Rule[] = REFERER_SITES.map((site, i) => ({
    id: DNR_RULE_BASE + i,
    priority: 1,
    condition: { requestDomains: site.domains, resourceTypes: ['xmlhttprequest'] },
    action: {
      type: 'modifyHeaders',
      requestHeaders: [{ header: 'Referer', operation: 'set', value: site.referer }],
    },
  }));
  refererRulesInstalled ??= browser.declarativeNetRequest
    .updateSessionRules({ removeRuleIds: rules.map((r) => r.id), addRules: rules })
    .catch((e: unknown) => {
      refererRulesInstalled = null; // 失败不缓存，下次保存重试
      throw e;
    });
  return refererRulesInstalled;
}

/** 下载前确保防盗链站点的 Referer 改写已生效（Firefox 为常驻监听，无需准备） */
async function ensureRefererRewrite(url: string): Promise<void> {
  if (!refererRewriteFor(url)) return;
  if (typeof browser.declarativeNetRequest?.updateSessionRules === 'function') {
    await installRefererRulesDnr();
  }
}

/** Firefox(MV2)：webRequest blocking 改写 Referer（仅站点表内域名命中） */
function rewriteRefererFirefox(details: { url: string; requestHeaders?: { name: string; value?: string }[] }) {
  const rule = refererRewriteFor(details.url);
  if (!rule) return {};
  const headers = (details.requestHeaders ?? []).filter((h) => h.name.toLowerCase() !== 'referer');
  headers.push({ name: 'Referer', value: rule.referer });
  return { requestHeaders: headers };
}

/** 浏览器网络栈下载并转 base64；分块拼接避免 String.fromCharCode 栈溢出，大图也在 service worker 可承受范围 */
async function downloadAsBase64(url: string): Promise<string> {
  await ensureRefererRewrite(url);
  const res = await fetch(url, { credentials: 'omit' });
  if (!res.ok) {
    throw new HttpError(res.status, `扩展下载失败：HTTP ${res.status}（${new URL(url).host}）`);
  }
  const bytes = new Uint8Array(await res.arrayBuffer());
  let binary = '';
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

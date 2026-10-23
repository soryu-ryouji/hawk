// 用户配置（~/.config/hawk/config.toml，全平台统一；当前库根的会话状态也收敛在此）。
// 目录只放应用自有配置：Electron 会话数据走平台默认 userData（main.ts 固定到 appData/hawk-app）。
import fs from 'node:fs';
import path from 'node:path';
import { parse, stringify } from 'smol-toml';
import { CONFIG_DIR } from './paths';
import type { CloseAction, LibraryHistoryItem, UpdateChannel } from './ipc-contract';

interface AppConfig {
  libraryPath?: string;
  libraryHistory?: string[];
  /** 冷启动模式记忆：true = 远程查看模式（无本地库，daemon 以无库形态启动）；开本地库时复位 */
  remoteViewer?: boolean;
  /** 全局缓存父目录（所有库共用，子目录按库名+哈希区分）；未设置时用系统缓存目录 */
  cacheParent?: string;
  /** 应用更新通道；未设置时视为 stable */
  updateChannel?: UpdateChannel;
  /** 关窗行为：exit=直接退出（默认），tray=隐藏到托盘驻留后台 */
  closeAction?: CloseAction;
  /** 本地 daemon 的 API 端口（默认 27371）：严格绑定，被占用由壳层询问用户换端口 */
  apiPort?: number;
}

export interface LibraryList {
  current: string | null;
  libraries: LibraryHistoryItem[];
}

const CONFIG_FILE = path.join(CONFIG_DIR, 'config.toml');

/** 本地 daemon 的 API 端口默认值与候选段。
 *  候选段是**跨仓库契约**：浏览器插件的本地模式按这一段顺序探活（`hawk-browser-extension/lib/api.ts`
 *  的 `LOCAL_PORT_RANGE`），「自动选择可用端口」只在这一段内挑，用户手动填到段外时插件需在高级选项里指定端口 */
export const DEFAULT_API_PORT = 27371;
export const API_PORT_RANGE: readonly [number, number] = [27371, 27400];

/** 配置默认值（唯一事实源：文件缺失落盘从这里序列化，读取回退也从这里兜底；
 *  有默认值的字段在此声明为必填，新增/调整字段只改 AppConfig 与此处，无独立模板文本可漂移） */
const DEFAULT_CONFIG: Readonly<{ updateChannel: UpdateChannel; closeAction: CloseAction }> = {
  updateChannel: 'stable',
  closeAction: 'exit',
};

/** 确保配置文件存在：缺失即把默认值序列化落盘，已存在则不动（不覆盖手改）。
 *  由 readConfig 惰性触发（含 writeConfig 的合并读），无需启动装配；字段含义见 AppConfig 的 JSDoc */
function ensureConfigFile(): void {
  try {
    if (fs.existsSync(CONFIG_FILE)) {
      return;
    }
    fs.mkdirSync(CONFIG_DIR, { recursive: true });
    fs.writeFileSync(CONFIG_FILE, stringify(DEFAULT_CONFIG));
  } catch {
    // 写失败按无配置继续启动（readConfig 回退默认值），不阻断主流程
  }
}

/** 去 undefined 字段（toml.stringify 不接受 undefined，丢弃语义与 JSON.stringify 对齐） */
function pruneUndefined(config: Partial<AppConfig>): Record<string, unknown> {
  return Object.fromEntries(Object.entries(config).filter(([, v]) => v !== undefined));
}

/** 当前素材库根目录（show-in-finder 的路径守卫要用）；会话级，换库时更新；
 *  远程查看模式（无本地库）为 null */
let libraryRoot: string | null = null;

export function getLibraryRoot(): string | null {
  return libraryRoot;
}

export function setLibraryRoot(root: string | null): void {
  libraryRoot = root;
}

export function readConfig(): AppConfig {
  ensureConfigFile();

  // 防止以下情况发生
  // 1. 文件存在，但解析异常，配置文件损坏
  // 2. ensure 创建文件有问题，文件配置实际仍然缺失，例如目录只读，磁盘已满
  // 3. 读写异常，例如文件被同步盘，用户杉树，或者其他例外情况
  try {
    return parse(fs.readFileSync(CONFIG_FILE, 'utf8')) as AppConfig;
  } catch {
    return {};
  }
}

export function writeConfig(patch: Partial<AppConfig>): void {
  fs.mkdirSync(CONFIG_DIR, { recursive: true });
  fs.writeFileSync(CONFIG_FILE, stringify(pruneUndefined({ ...readConfig(), ...patch })));
}

/** 更新通道偏好（config.toml 持久化；未设置/损坏回退默认值） */
export function getUpdateChannel(): UpdateChannel {
  return readConfig().updateChannel ?? DEFAULT_CONFIG.updateChannel;
}

export function setUpdateChannel(channel: UpdateChannel): void {
  writeConfig({ updateChannel: channel });
}

/** 关窗行为偏好（config.toml 持久化；未设置/损坏回退默认值）。关窗时实时读取，改后无需重启 */
export function getCloseAction(): CloseAction {
  return readConfig().closeAction ?? DEFAULT_CONFIG.closeAction;
}

export function setCloseAction(action: CloseAction): void {
  writeConfig({ closeAction: action });
}

/** 本地 API 端口：配置值合法则用之，否则回退默认端口（手改成非法值时不让 app 起不来） */
export function getApiPort(): number {
  const port = readConfig().apiPort;
  return typeof port === 'number' && Number.isInteger(port) && port >= 1024 && port <= 65535 ? port : DEFAULT_API_PORT;
}

export function setApiPort(port: number): void {
  writeConfig({ apiPort: port });
}

/** 库显示名：优先读库内 .hawk/config.toml 的 name（与 daemon library/info 同源），缺失/异常回退目录名 */
function libraryDisplayName(libPath: string): string {
  try {
    const text = fs.readFileSync(path.join(libPath, '.hawk', 'config.toml'), 'utf8');
    const name = (parse(text) as { name?: unknown }).name;
    if (typeof name === 'string' && name.trim()) {
      return name.trim();
    }
  } catch {
    // 无配置文件/解析失败：回退目录名
  }
  return path.basename(libPath);
}

/** 历史库列表（最近使用在前，含目录存在性；当前库由 libraryRoot 标记） */
export function listLibraries(): LibraryList {
  const history = readConfig().libraryHistory ?? [];
  return {
    current: libraryRoot,
    libraries: history.filter((p): p is string => typeof p === 'string').map((p) => ({ path: p, name: libraryDisplayName(p), exists: fs.existsSync(p) })),
  };
}

/** 从历史中移除一条素材库记录（不动目录本身；当前库由 libraryRoot 会话标记，不受历史删减影响） */
export function removeLibraryHistory(libPath: string): LibraryList {
  const history = (readConfig().libraryHistory ?? []).filter((p) => p !== libPath);
  writeConfig({ libraryHistory: history });
  return listLibraries();
}

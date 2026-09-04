// 用户配置（~/.config/hawk/config.toml，全平台统一；当前库根的会话状态也收敛在此）。
// 目录只放应用自有配置：Electron 会话数据走平台默认 userData（main.ts 固定到 appData/hawk-app）。
import fs from 'node:fs';
import path from 'node:path';
import { parse, stringify } from 'smol-toml';
import { CONFIG_DIR } from './paths';
import type { LibraryHistoryItem } from './ipc-contract';

interface AppConfig {
  libraryPath?: string;
  libraryHistory?: string[];
  /** 全局缓存父目录（所有库共用，子目录按库名+哈希区分）；未设置时用系统缓存目录 */
  cacheParent?: string;
}

export interface LibraryList {
  current: string | null;
  libraries: LibraryHistoryItem[];
}

const CONFIG_FILE = path.join(CONFIG_DIR, 'config.toml');

/** 去 undefined 字段（toml.stringify 不接受 undefined，丢弃语义与 JSON.stringify 对齐） */
function pruneUndefined(config: Partial<AppConfig>): Record<string, unknown> {
  return Object.fromEntries(Object.entries(config).filter(([, v]) => v !== undefined));
}

/** 当前素材库根目录（show-in-finder 的路径守卫要用）；会话级，换库时更新 */
let libraryRoot: string | null = null;

export function getLibraryRoot(): string | null {
  return libraryRoot;
}

export function setLibraryRoot(root: string): void {
  libraryRoot = root;
}

export function readConfig(): AppConfig {
  try {
    return parse(fs.readFileSync(CONFIG_FILE, 'utf8')) as AppConfig;
  } catch {
    return {}; // 无配置/损坏：回退空配置，由调用方走引导流程
  }
}

export function writeConfig(patch: Partial<AppConfig>): void {
  fs.mkdirSync(CONFIG_DIR, { recursive: true });
  fs.writeFileSync(CONFIG_FILE, stringify(pruneUndefined({ ...readConfig(), ...patch })));
}

/** 历史库列表（最近使用在前，含目录存在性；当前库由 libraryRoot 标记） */
export function listLibraries(): LibraryList {
  const history = readConfig().libraryHistory ?? [];
  return {
    current: libraryRoot,
    libraries: history
      .filter((p): p is string => typeof p === 'string')
      .map((p) => ({ path: p, name: path.basename(p), exists: fs.existsSync(p) })),
  };
}

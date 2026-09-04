// 用户配置（~/.config/hawk/hawk-app.json，全平台统一；当前库根的会话状态也收敛在此）。
// userData 已由 main.ts 重定向（app.setPath），此处只管读写与旧位置迁移。
import { app } from 'electron';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
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

const CONFIG_FILE = (): string => path.join(app.getPath('userData'), 'hawk-app.json');

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
    return JSON.parse(fs.readFileSync(CONFIG_FILE(), 'utf8')) as AppConfig;
  } catch {
    return {};
  }
}

export function writeConfig(patch: Partial<AppConfig>): void {
  fs.writeFileSync(CONFIG_FILE(), JSON.stringify({ ...readConfig(), ...patch }, null, 2));
}

/** 一次性迁移：旧平台默认位置的 hawk-app.json → ~/.config/hawk（新位置缺失时）。
 *  Windows 旧位置为 %APPDATA%/hawk（打包）与 %APPDATA%/hawk-app（开发态）；macOS 为
 *  ~/Library/Application Support/hawk；Linux 旧默认即新位置，无需迁移 */
export function migrateLegacyConfig(): void {
  const target = CONFIG_FILE();
  if (fs.existsSync(target)) {
    return;
  }
  const home = os.homedir();
  const legacyDirs: string[] = [];
  if (process.platform === 'win32') {
    const roaming = process.env.APPDATA || path.join(home, 'AppData', 'Roaming');
    legacyDirs.push(path.join(roaming, 'hawk'), path.join(roaming, 'hawk-app'));
  } else if (process.platform === 'darwin') {
    legacyDirs.push(
      path.join(home, 'Library', 'Application Support', 'hawk'),
      path.join(home, 'Library', 'Application Support', 'hawk-app'),
    );
  }
  for (const dir of legacyDirs) {
    const legacyFile = path.join(dir, 'hawk-app.json');
    if (fs.existsSync(legacyFile)) {
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.copyFileSync(legacyFile, target);
      return;
    }
  }
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

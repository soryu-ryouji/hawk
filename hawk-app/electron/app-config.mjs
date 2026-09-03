// 用户配置（userData/hawk-app.json）：记住上次素材库与历史记录；当前库根的会话状态也收敛在此。
import { app } from 'electron';
import fs from 'node:fs';
import path from 'node:path';

const CONFIG_FILE = () => path.join(app.getPath('userData'), 'hawk-app.json');

/** 当前素材库根目录（show-in-finder 的路径守卫要用）；会话级，换库时更新 */
let libraryRoot = null;

export function getLibraryRoot() {
  return libraryRoot;
}

export function setLibraryRoot(root) {
  libraryRoot = root;
}

export function readConfig() {
  try {
    return JSON.parse(fs.readFileSync(CONFIG_FILE(), 'utf8'));
  } catch {
    return {};
  }
}

export function writeConfig(patch) {
  fs.writeFileSync(CONFIG_FILE(), JSON.stringify({ ...readConfig(), ...patch }, null, 2));
}

/** 历史库列表（最近使用在前，含目录存在性；当前库由 libraryRoot 标记） */
export function listLibraries() {
  const history = readConfig().libraryHistory ?? [];
  return {
    current: libraryRoot,
    libraries: history
      .filter((p) => typeof p === 'string')
      .map((p) => ({ path: p, name: path.basename(p), exists: fs.existsSync(p) })),
  };
}

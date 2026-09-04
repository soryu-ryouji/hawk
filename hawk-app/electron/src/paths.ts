// 共用路径：产物位于 electron/out/（esbuild 打包），hawk-app 根需上溯两级；应用图标位置单一来源。
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

/** 产物目录（electron/out） */
export const ELECTRON_DIR = path.dirname(fileURLToPath(import.meta.url));

/** hawk-app 包根（electron/out 上溯两级） */
export const APP_DIR = path.join(ELECTRON_DIR, '..', '..');

/** 窗口/托盘共用的应用图标（build/icon.png，512px 源图，托盘用时按平台重采样） */
export const APP_ICON = path.join(APP_DIR, 'build', 'icon.png');

/** 全局配置目录（全平台统一 ~/.config/hawk；经 app.setPath('userData') 重定向，
 *  hawk-app.json 与 Electron 会话数据（localStorage 等）均落在此） */
export const USER_DATA_DIR = path.join(os.homedir(), '.config', 'hawk');

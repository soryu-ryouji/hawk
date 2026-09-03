// 共用路径：ESM 无 __dirname，各模块统一从这里取；窗口/托盘图标位置单一来源。
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const ELECTRON_DIR = path.dirname(fileURLToPath(import.meta.url));

/** 窗口/托盘共用的应用图标（build/icon.png，512px 源图，托盘用时按平台重采样） */
export const APP_ICON = path.join(ELECTRON_DIR, '..', 'build', 'icon.png');

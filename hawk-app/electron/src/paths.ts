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

/** 应用自有配置目录（全平台统一 ~/.config/hawk；只放 TOML 配置，与 Electron 会话数据分离）。
 *  Electron 平台默认 userData 不可直接用：打包版 productName=hawk 会让 Linux 默认 userData
 *  恰好解析为本目录，Chromium 缓存会混进来——main.ts 把 userData 固定到 appData/hawk-app */
export const CONFIG_DIR = path.join(os.homedir(), '.config', 'hawk');

/** 应用级远程访问配置（跨素材库共享；daemon 读写，设备身份在其同级 remote/ 目录）。
 *  与 config.toml（壳偏好）分开：远程配置由 daemon 权威写入，避免两个进程争同一文件 */
export const REMOTE_CONFIG_FILE = path.join(CONFIG_DIR, 'remote.toml');

/** daemon stdout（tracing 日志）落盘位置：每次拉起截断重写，只留当次会话；远程打洞/中继排障依赖 */
export const DAEMON_LOG_FILE = path.join(CONFIG_DIR, 'daemon.log');

/** 已废弃的远程查看宿主库目录（旧版在本地建假库承载 daemon；现已由 daemon 无库模式取代）。
 *  仅存留一处用途：冷启动时识别旧配置并转为远程查看模式（不再启动该目录） */
export const LEGACY_REMOTE_HOST_DIR = path.join(CONFIG_DIR, 'remote-library');

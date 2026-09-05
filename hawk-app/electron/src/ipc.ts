// 白名单 IPC：窗口控制、素材库选择/历史、局域网地址、文件管理器/剪贴板、退出。
// 业务数据一律走 REST，不经 IPC（更新通道的注册在 updater.ts）。
import { app, clipboard, dialog, ipcMain, shell } from 'electron';
import fs from 'node:fs';
import path from 'node:path';
import { getMainWindow, setQuitting } from './window';
import { getCloseAction, getLibraryRoot, listLibraries, removeLibraryHistory, setCloseAction } from './app-config';
import { openLibraryAt, pickLibrary, getStartedConn } from './server';
import { changeCacheParent, currentCacheParent } from './cache';
import { lanAddresses } from './lan';
import { IPC } from './ipc-contract';

export function registerIpc(): void {
  // 真正退出应用（启动错误屏的「退出 hawk」按钮）：置放行标志后退出，回收 server
  ipcMain.handle(IPC.quitApp, () => {
    setQuitting();
    app.quit();
  });

  // 自绘标题栏的窗口控制（无边框窗口没有原生按钮）
  ipcMain.handle(IPC.winMinimize, () => getMainWindow()?.minimize());
  ipcMain.handle(IPC.winMaximizeToggle, () => {
    const win = getMainWindow();
    if (!win) {
      return false;
    }
    if (win.isMaximized()) {
      win.unmaximize();
    } else {
      win.maximize();
    }
    return win.isMaximized();
  });
  ipcMain.handle(IPC.winClose, () => getMainWindow()?.close());

  // 关窗行为偏好（config.toml 持久化，主进程为唯一事实来源；渲染层经 IPC 读写）
  ipcMain.handle(IPC.closeActionGet, () => getCloseAction());
  ipcMain.handle(IPC.closeActionSet, (_event, action: string) => {
    if (action !== 'exit' && action !== 'tray') {
      throw new Error('未知关窗行为');
    }
    setCloseAction(action);
  });

  ipcMain.handle(IPC.selectLibrary, async (): Promise<boolean> => {
    const selected = await pickLibrary();
    if (!selected) {
      return false;
    }
    try {
      // 端口/token 即选即生成；页面切应用内启动屏，就绪经 hawk:server-started 通知
      await openLibraryAt(selected);
      return true;
    } catch (error) {
      // 失败时留在引导页并给出可见错误，而不是让 IPC 静默 reject
      dialog.showErrorBox('hawk-daemon 启动失败', String(error instanceof Error ? error.message : error));
      return false;
    }
  });

  ipcMain.handle(IPC.listLibraries, () => listLibraries());

  ipcMain.handle(IPC.openLibrary, async (_event, libPath: unknown): Promise<boolean> => {
    // 只接受历史记录内的路径（与目录选择框等效的白名单）
    if (typeof libPath !== 'string' || !listLibraries().libraries.some((l) => l.path === libPath)) {
      return false;
    }
    try {
      await openLibraryAt(libPath);
      return true;
    } catch (error) {
      dialog.showErrorBox('hawk-daemon 启动失败', String(error instanceof Error ? error.message : error));
      return false;
    }
  });

  ipcMain.handle(IPC.openLibraryFolder, (_event, libPath: unknown) => {
    // 与 openLibrary 同规约：只接受历史记录内的路径（打开目录本身，非 reveal 到父目录）
    if (typeof libPath !== 'string' || !listLibraries().libraries.some((l) => l.path === libPath)) {
      return;
    }
    void shell.openPath(libPath);
  });

  ipcMain.handle(IPC.removeLibrary, (_event, libPath: unknown) => {
    // 与 openLibrary 同规约：只接受历史记录内的路径
    if (typeof libPath !== 'string' || !listLibraries().libraries.some((l) => l.path === libPath)) {
      return listLibraries();
    }
    return removeLibraryHistory(libPath);
  });

  // 缓存目录（设置面板「存储」分区）：查询当前值 / 选目录 / 校验并迁移（内部停旧 server、搬迁、重启）
  ipcMain.handle(IPC.cacheDirGet, () => currentCacheParent());
  ipcMain.handle(IPC.cacheDirPick, async () => {
    const win = getMainWindow();
    if (!win) {
      return null;
    }
    const result = await dialog.showOpenDialog(win, {
      title: '选择缓存目录',
      properties: ['openDirectory', 'createDirectory'],
    });
    return result.canceled ? null : result.filePaths[0];
  });
  ipcMain.handle(IPC.cacheDirChange, async (_event, dir: unknown) => {
    if (typeof dir !== 'string') {
      return '非法目录';
    }
    return changeCacheParent(dir);
  });

  ipcMain.handle(IPC.lanAddresses, () => lanAddresses());

  // 当前已就绪的 server 连接（未就绪 null）：页面（重）加载晚于 server-started 事件时的竞态兜底
  ipcMain.handle(IPC.serverConn, () => getStartedConn());

  ipcMain.handle(IPC.showInFinder, (_event, relPath: unknown) => {
    const abs = resolveLibraryPath(relPath);
    if (abs) {
      shell.showItemInFolder(abs);
    }
  });

  // 打开库内文件夹本身（侧栏文件夹右键；区别 showInFinder 的定位到父级）
  ipcMain.handle(IPC.openFolder, (_event, relPath: unknown) => {
    const abs = resolveLibraryPath(relPath);
    if (abs && fs.existsSync(abs) && fs.statSync(abs).isDirectory()) {
      void shell.openPath(abs);
    }
  });

  // 重启当前库的 server（存储方案迁移等设置变更后调用；switchLibrary 会先切启动屏再重建）
  ipcMain.handle(IPC.restartServer, async () => {
    const libRoot = getLibraryRoot();
    if (libRoot) {
      await openLibraryAt(libRoot);
    }
  });

  // 复制文件路径到剪贴板（预览右键菜单；复制图片在渲染进程经 Web Clipboard API 完成，无 IPC）
  ipcMain.handle(IPC.copyPath, async (_event, relPath: unknown) => {
    const abs = resolveLibraryPath(relPath);
    if (abs) {
      await clipboard.writeText(abs);
    }
  });
}

/** 库内相对路径 → 绝对路径（含越界守卫），非法路径返回 null */
function resolveLibraryPath(relPath: unknown): string | null {
  const libraryRoot = getLibraryRoot();
  if (typeof relPath !== 'string' || relPath.includes('..') || !libraryRoot) {
    return null;
  }
  const abs = path.join(libraryRoot, ...relPath.split('/'));
  return path.resolve(abs).startsWith(path.resolve(libraryRoot)) ? abs : null;
}

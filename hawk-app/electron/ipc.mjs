// 白名单 IPC：窗口控制、素材库选择/历史、局域网地址、文件管理器/剪贴板、退出。
// 业务数据一律走 REST，不经 IPC（更新通道的注册在 updater.mjs）。
import { app, clipboard, dialog, ipcMain, nativeImage, shell } from 'electron';
import path from 'node:path';
import { getMainWindow, setQuitting } from './window.mjs';
import { getLibraryRoot, listLibraries } from './app-config.mjs';
import { openLibraryAt, pickLibrary } from './server.mjs';
import { lanAddresses } from './lan.mjs';

export function registerIpc() {
  // 真正退出应用（启动错误屏的「退出 hawk」按钮）：置放行标志后退出，回收 server
  ipcMain.handle('hawk:quit-app', () => {
    setQuitting();
    app.quit();
  });

  // 自绘标题栏的窗口控制（无边框窗口没有原生按钮）
  ipcMain.handle('hawk:win-minimize', () => getMainWindow()?.minimize());
  ipcMain.handle('hawk:win-maximize-toggle', () => {
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
  ipcMain.handle('hawk:win-close', () => getMainWindow()?.close());

  ipcMain.handle('hawk:select-library', async () => {
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
      dialog.showErrorBox('hawk-daemon 启动失败', String(error && error.message ? error.message : error));
      return false;
    }
  });

  ipcMain.handle('hawk:list-libraries', () => listLibraries());

  ipcMain.handle('hawk:open-library', async (_event, libPath) => {
    // 只接受历史记录内的路径（与目录选择框等效的白名单）
    if (typeof libPath !== 'string' || !listLibraries().libraries.some((l) => l.path === libPath)) {
      return false;
    }
    try {
      await openLibraryAt(libPath);
      return true;
    } catch (error) {
      dialog.showErrorBox('hawk-daemon 启动失败', String(error && error.message ? error.message : error));
      return false;
    }
  });

  ipcMain.handle('hawk:lan-addresses', () => lanAddresses());

  ipcMain.handle('hawk:show-in-finder', (_event, relPath) => {
    const abs = resolveLibraryPath(relPath);
    if (abs) {
      shell.showItemInFolder(abs);
    }
  });

  // 复制文件路径/图片本体到剪贴板（预览右键菜单）
  ipcMain.handle('hawk:copy-path', (_event, relPath) => {
    const abs = resolveLibraryPath(relPath);
    if (abs) {
      clipboard.writeText(abs);
    }
  });

  ipcMain.handle('hawk:copy-image', (_event, relPath) => {
    const abs = resolveLibraryPath(relPath);
    if (!abs) {
      return;
    }
    const image = nativeImage.createFromPath(abs);
    if (!image.isEmpty()) {
      clipboard.writeImage(image);
    }
  });
}

/** 库内相对路径 → 绝对路径（含越界守卫），非法路径返回 null */
function resolveLibraryPath(relPath) {
  const libraryRoot = getLibraryRoot();
  if (typeof relPath !== 'string' || relPath.includes('..') || !libraryRoot) {
    return null;
  }
  const abs = path.join(libraryRoot, ...relPath.split('/'));
  return path.resolve(abs).startsWith(path.resolve(libraryRoot)) ? abs : null;
}

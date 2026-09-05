// 窗口与系统托盘：主窗口生命周期（关窗行为可配：直接退出/隐藏到托盘）、托盘菜单、最大化状态同步、退出标志。
import { app, BrowserWindow, Menu, nativeImage, Tray } from 'electron';
import path from 'node:path';
import fs from 'node:fs';
import { APP_DIR, APP_ICON, ELECTRON_DIR } from './paths';
import { getCloseAction } from './app-config';
import { IPC, type ServerConn } from './ipc-contract';

const isDev = !app.isPackaged;

let mainWindow: BrowserWindow | null = null;
/** 系统托盘实例（必须常驻引用，否则会被 GC 回收导致托盘消失） */
let tray: Tray | null = null;
/** 真正退出标志：托盘菜单「退出」/ macOS Cmd+Q 时置位，放行 close 拦截 */
let isQuitting = false;

export function getMainWindow(): BrowserWindow | null {
  return mainWindow;
}

export function setQuitting(): void {
  isQuitting = true;
}

/** 从托盘/二次启动唤起主窗口 */
export function showMainWindow(): void {
  if (!mainWindow) {
    return;
  }
  if (mainWindow.isMinimized()) {
    mainWindow.restore();
  }
  mainWindow.show();
  mainWindow.focus();
}

export function loadMainPage(conn?: ServerConn): void {
  const hash = conn ? `api=${encodeURIComponent(conn.address)}&token=${conn.token}` : '';
  if (isDev) {
    mainWindow?.loadURL(`http://localhost:5173/${hash ? `#${hash}` : ''}`);
  } else {
    mainWindow?.loadFile(path.join(APP_DIR, 'web', 'dist', 'index.html'), { hash });
  }
}

export function createWindow(): void {
  mainWindow = new BrowserWindow({
    width: 1440,
    height: 900,
    minWidth: 960,
    minHeight: 600,
    show: false,
    backgroundColor: '#1e1e1e',
    // macOS：隐藏系统标题栏但保留原生红绿灯（悬停 glyph、失焦置灰、全屏行为由系统保证），
    // trafficLightPosition 按 40px 标题栏垂直居中；Windows/Linux：无边框，窗口控制由前端自绘
    ...(process.platform === 'darwin'
      ? { titleBarStyle: 'hidden' as const, trafficLightPosition: { x: 12, y: 14 } }
      : { frame: false }),
    // 开发态 / Linux 的窗口图标；打包后各平台图标由 electron-builder 嵌入
    icon: APP_ICON,
    webPreferences: {
      // 产物 preload.cjs 与 main.mjs 同目录（electron/out）
      preload: path.join(ELECTRON_DIR, 'preload.cjs'),
    },
  });
  const win = mainWindow;
  // 首帧渲染完成后再显示窗口：GPU 驱动不认可 backgroundColor、合成器首帧延迟（秒级）时，
  // 提前 show 会把空白/白窗暴露给用户——ready-to-show 是「内容已可见」的可靠信号
  win.once('ready-to-show', () => win.show());
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  // 关窗行为按偏好分流（实时读取 config.toml，设置面板改后即生效）：
  // tray：拦截 close 改为隐藏到托盘（Eagle 式驻留）；exit（默认）：放行 close，
  // 由 window-all-closed 退出应用。真正退出（托盘菜单/macOS Cmd+Q）经 before-quit 置 isQuitting 放行
  win.on('close', (event) => {
    if (isQuitting) {
      return;
    }
    if (getCloseAction() === 'tray') {
      event.preventDefault();
      win.hide();
    }
  });
  // 同步最大化状态给渲染进程（标题栏 最大化/还原 图标切换）
  win.on('maximize', () => win.webContents.send(IPC.winMaximized, true));
  win.on('unmaximize', () => win.webContents.send(IPC.winMaximized, false));
  // 窗口内容单页生命周期：启动/引导/进度全在页面内呈现，主进程不再驱动二次导航

  // 无头自检：HAWK_SCREENSHOT=<路径> 时加载完成后截图落盘
  if (process.env.HAWK_SCREENSHOT) {
    win.webContents.once('did-finish-load', () => {
      const delay = Number(process.env.HAWK_SCREENSHOT_DELAY || 5000);
      setTimeout(async () => {
        const image = await win.webContents.capturePage();
        fs.writeFileSync(process.env.HAWK_SCREENSHOT as string, image.toPNG());
        console.log(`screenshot saved: ${process.env.HAWK_SCREENSHOT}`);
      }, delay);
    });
  }
}

// ---------- 系统托盘（Eagle 式：关窗不退出，驻留后台，hawk-daemon 继续服务浏览器扩展采集） ----------

export function createTray(): void {
  const image = nativeImage.createFromPath(APP_ICON);
  if (image.isEmpty()) {
    console.warn('tray icon missing, tray disabled:', APP_ICON);
    return;
  }
  // Windows/Linux 托盘 16–32px，macOS 菜单栏约 18px
  const size = process.platform === 'darwin' ? 18 : 32;
  tray = new Tray(image.resize({ width: size, height: size }));
  tray.setToolTip('hawk');
  tray.setContextMenu(
    Menu.buildFromTemplate([
      { label: '打开 hawk', click: showMainWindow },
      { type: 'separator' },
      { label: '退出', click: () => app.quit() },
    ]),
  );
  // 左键单击托盘图标唤起（Windows/Linux 惯例；macOS 上以右键菜单为主，左键唤起也无碍）
  tray.on('click', showMainWindow);
}

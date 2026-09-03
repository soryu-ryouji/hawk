// 窗口与系统托盘：主窗口生命周期（关窗隐藏到托盘）、托盘菜单、最大化状态同步、退出标志。
import { app, BrowserWindow, Menu, nativeImage, Tray } from 'electron';
import path from 'node:path';
import fs from 'node:fs';
import { APP_ICON, ELECTRON_DIR } from './paths.mjs';

const isDev = !app.isPackaged;

/** @type {BrowserWindow | null} */
let mainWindow = null;
/** 系统托盘实例（必须常驻引用，否则会被 GC 回收导致托盘消失） */
let tray = null;
/** 真正退出标志：托盘菜单「退出」/ macOS Cmd+Q 时置位，放行 close 拦截 */
let isQuitting = false;

export function getMainWindow() {
  return mainWindow;
}

export function getIsQuitting() {
  return isQuitting;
}

export function setQuitting() {
  isQuitting = true;
}

/** 从托盘/二次启动唤起主窗口 */
export function showMainWindow() {
  if (!mainWindow) {
    return;
  }
  if (mainWindow.isMinimized()) {
    mainWindow.restore();
  }
  mainWindow.show();
  mainWindow.focus();
}

export function loadMainPage(conn) {
  const hash = conn ? `api=${encodeURIComponent(conn.address)}&token=${conn.token}` : '';
  if (isDev) {
    mainWindow.loadURL(`http://localhost:5173/${hash ? `#${hash}` : ''}`);
  } else {
    mainWindow.loadFile(path.join(ELECTRON_DIR, '..', 'web', 'dist', 'index.html'), { hash });
  }
}

export function createWindow() {
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
      ? { titleBarStyle: 'hidden', trafficLightPosition: { x: 12, y: 14 } }
      : { frame: false }),
    // 开发态 / Linux 的窗口图标；打包后各平台图标由 electron-builder 嵌入
    icon: APP_ICON,
    webPreferences: {
      preload: path.join(ELECTRON_DIR, 'preload.cjs'),
    },
  });
  // 首帧渲染完成后再显示窗口：GPU 驱动不认可 backgroundColor、合成器首帧延迟（秒级）时，
  // 提前 show 会把空白/白窗暴露给用户——ready-to-show 是「内容已可见」的可靠信号
  mainWindow.once('ready-to-show', () => mainWindow?.show());
  mainWindow.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  // Eagle 式关窗：拦截 close 改为隐藏到托盘（真正退出经托盘菜单，见 before-quit 的 isQuitting）
  mainWindow.on('close', (event) => {
    if (isQuitting) {
      return;
    }
    event.preventDefault();
    mainWindow.hide();
  });
  // 同步最大化状态给渲染进程（标题栏 最大化/还原 图标切换）
  mainWindow.on('maximize', () => mainWindow?.webContents.send('hawk:win-maximized', true));
  mainWindow.on('unmaximize', () => mainWindow?.webContents.send('hawk:win-maximized', false));
  // 窗口内容单页生命周期：启动/引导/进度全在页面内呈现，主进程不再驱动二次导航

  // 无头自检：HAWK_SCREENSHOT=<路径> 时加载完成后截图落盘
  if (process.env.HAWK_SCREENSHOT) {
    mainWindow.webContents.once('did-finish-load', () => {
      const delay = Number(process.env.HAWK_SCREENSHOT_DELAY || 5000);
      setTimeout(async () => {
        const image = await mainWindow.webContents.capturePage();
        fs.writeFileSync(process.env.HAWK_SCREENSHOT, image.toPNG());
        console.log(`screenshot saved: ${process.env.HAWK_SCREENSHOT}`);
      }, delay);
    });
  }
}

// ---------- 系统托盘（Eagle 式：关窗不退出，驻留后台，hawk-daemon 继续服务浏览器扩展采集） ----------

export function createTray() {
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

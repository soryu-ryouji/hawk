// hawk-app Electron 主进程：窗口管理（关窗隐藏到托盘）、系统托盘、单实例、拉起/回收 hawk-server、token 注入、库选择、白名单 IPC。
// 业务数据一律走 REST，不经 IPC（见 docs/architecture.md、docs/hawk-app.md）。
const { app, BrowserWindow, dialog, ipcMain, shell, Menu, Tray, nativeImage } = require('electron');
const { spawn } = require('node:child_process');
const path = require('node:path');
const fs = require('node:fs');
const crypto = require('node:crypto');

const isDev = !app.isPackaged;
const CONFIG_FILE = () => path.join(app.getPath('userData'), 'hawk-app.json');

/** @type {{ child: import('child_process').ChildProcess, address: string, token: string } | null} */
let server = null;
/** @type {BrowserWindow | null} */
let mainWindow = null;
/** 系统托盘实例（必须常驻引用，否则会被 GC 回收导致托盘消失） */
let tray = null;
/** 真正退出标志：托盘菜单「退出」/ macOS Cmd+Q 时置位，放行 close 拦截 */
let isQuitting = false;
/** 当前素材库根目录（show-in-finder 的路径守卫要用） */
let libraryRoot = null;

// 窗口/托盘共用的应用图标（build/icon.png，512px 源图，托盘用时按平台重采样）
const APP_ICON = path.join(__dirname, '..', 'build', 'icon.png');

// 单实例：托盘驻留期间再次启动（双击图标/快捷方式）应唤起已有窗口，而不是拉起第二个实例
// （第二个实例的 hawk-server 会因 27371 端口占用直接启动失败）
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', showMainWindow);
}

// ---------- 用户配置（记住上次素材库） ----------

function readConfig() {
  try {
    return JSON.parse(fs.readFileSync(CONFIG_FILE(), 'utf8'));
  } catch {
    return {};
  }
}

function writeConfig(patch) {
  fs.writeFileSync(CONFIG_FILE(), JSON.stringify({ ...readConfig(), ...patch }, null, 2));
}

// ---------- hawk-server 进程管理 ----------

function resolveServerCommand() {
  if (process.env.HAWK_SERVER_EXE) {
    return { command: process.env.HAWK_SERVER_EXE, args: [] };
  }
  if (isDev) {
    // 开发态：直接 dotnet 运行本地构建产物
    const dll = path.join(__dirname, '..', '..', 'hawk-server', 'bin', 'Debug', 'net10.0', 'hawk-server.dll');
    return { command: 'dotnet', args: [dll] };
  }
  // 打包态：extraResources 携带的自包含二进制
  const bin = process.platform === 'win32' ? 'hawk-server.exe' : 'hawk-server';
  return { command: path.join(process.resourcesPath, 'hawk-server', bin), args: [] };
}

/** 拉起 hawk-server，解析 stdout 的 HAWK_READY 行取得实际监听地址 */
function startServer(libPath) {
  const { command, args } = resolveServerCommand();
  const token = crypto.randomBytes(32).toString('hex');
  const child = spawn(command, [...args, '--library', libPath, '--port', '27371'], {
    env: { ...process.env, HAWK_TOKEN: token },
    stdio: ['ignore', 'pipe', 'pipe'],
    // GUI 进程拉起控制台子进程：不隐藏会在 Windows 上弹出黑窗口
    windowsHide: true,
  });

  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('hawk-server 启动超时')), 60000);
    let buffer = '';
    let stderrTail = '';
    child.stdout.on('data', (chunk) => {
      buffer += chunk.toString();
      const match = buffer.match(/HAWK_READY (\S+) token=(\w+)/);
      if (match) {
        clearTimeout(timer);
        resolve({ child, address: match[1], token });
      }
    });
    // 留 stderr 尾部用于报错；开发态同时转发到终端
    child.stderr.on('data', (chunk) => {
      stderrTail = (stderrTail + chunk.toString()).slice(-4000);
      if (isDev) {
        process.stderr.write(chunk);
      }
    });
    child.on('error', (error) => {
      clearTimeout(timer);
      reject(new Error(`hawk-server 启动失败: ${error.message}`));
    });
    child.on('exit', (code) => {
      clearTimeout(timer);
      const detail = stderrTail.trim();
      reject(new Error(`hawk-server 启动失败（退出码 ${code}）${detail ? `\n${detail}` : ''}`));
    });
  });
}

function stopServer() {
  if (server && !server.child.killed) {
    server.child.kill();
  }
  server = null;
}

// ---------- 窗口 ----------

/** 从托盘/二次启动唤起主窗口 */
function showMainWindow() {
  if (!mainWindow) {
    return;
  }
  if (mainWindow.isMinimized()) {
    mainWindow.restore();
  }
  mainWindow.show();
  mainWindow.focus();
}

function loadMainPage() {
  const hash = `api=${encodeURIComponent(server.address)}&token=${server.token}`;
  if (isDev) {
    mainWindow.loadURL(`http://localhost:5173/#${hash}`);
  } else {
    mainWindow.loadFile(path.join(__dirname, '..', 'web', 'dist', 'index.html'), { hash });
  }
}

/** 未配置素材库时的引导页：不带连接参数，由页面按钮触发目录选择框 */
function loadSetupPage() {
  if (isDev) {
    mainWindow.loadURL('http://localhost:5173/');
  } else {
    mainWindow.loadFile(path.join(__dirname, '..', 'web', 'dist', 'index.html'));
  }
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1440,
    height: 900,
    minWidth: 960,
    minHeight: 600,
    backgroundColor: '#1e1e1e',
    // macOS：隐藏系统标题栏但保留原生红绿灯（悬停 glyph、失焦置灰、全屏行为由系统保证），
    // trafficLightPosition 按 40px 标题栏垂直居中；Windows/Linux：无边框，窗口控制由前端自绘
    ...(process.platform === 'darwin'
      ? { titleBarStyle: 'hidden', trafficLightPosition: { x: 12, y: 14 } }
      : { frame: false }),
    // 开发态 / Linux 的窗口图标；打包后各平台图标由 electron-builder 嵌入
    icon: APP_ICON,
    webPreferences: {
      preload: path.join(__dirname, 'preload.cjs'),
    },
  });
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
  // server 未就绪前不加载页面；启动/换库流程在 server 就绪后调用 loadMainPage

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

// ---------- 系统托盘（Eagle 式：关窗不退出，驻留后台，hawk-server 继续服务浏览器扩展采集） ----------

function createTray() {
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

// ---------- 素材库选择 ----------

async function pickLibrary() {
  const result = await dialog.showOpenDialog(mainWindow, {
    title: '选择素材库目录',
    properties: ['openDirectory', 'createDirectory'],
  });
  return result.canceled ? null : result.filePaths[0];
}

async function switchLibrary(libPath) {
  stopServer();
  writeConfig({ libraryPath: libPath });
  libraryRoot = libPath;
  server = await startServer(libPath);
}

// ---------- 白名单 IPC ----------

// 自绘标题栏的窗口控制（无边框窗口没有原生按钮）
ipcMain.handle('hawk:win-minimize', () => mainWindow?.minimize());
ipcMain.handle('hawk:win-maximize-toggle', () => {
  if (!mainWindow) {
    return false;
  }
  if (mainWindow.isMaximized()) {
    mainWindow.unmaximize();
  } else {
    mainWindow.maximize();
  }
  return mainWindow.isMaximized();
});
ipcMain.handle('hawk:win-close', () => mainWindow?.close());

ipcMain.handle('hawk:select-library', async () => {
  const selected = await pickLibrary();
  if (!selected) {
    return false;
  }
  try {
    await switchLibrary(selected);
  } catch (error) {
    // 失败时留在当前页并给出可见错误，而不是让 IPC 静默 reject
    dialog.showErrorBox('hawk-server 启动失败', String(error && error.message ? error.message : error));
    return false;
  }
  loadMainPage();
  return true;
});

ipcMain.handle('hawk:show-in-finder', (_event, relPath) => {
  if (typeof relPath !== 'string' || relPath.includes('..')) {
    return;
  }
  const abs = path.join(libraryRoot, ...relPath.split('/'));
  if (!path.resolve(abs).startsWith(path.resolve(libraryRoot))) {
    return;
  }
  shell.showItemInFolder(abs);
});

// ---------- 生命周期 ----------

app.whenReady().then(async () => {
  createWindow();
  createTray();

  const libPath = readConfig().libraryPath;
  if (!libPath || !fs.existsSync(libPath)) {
    loadSetupPage(); // 素材库未配置或已失效：进引导页，不再直接弹目录选择框
    return;
  }

  try {
    await switchLibrary(libPath);
  } catch (error) {
    dialog.showErrorBox('hawk-server 启动失败', String(error));
    app.quit();
    return;
  }
  loadMainPage(); // 首次 createWindow 时 server 未就绪，就绪后重新加载带 token 的地址
});

// 关窗只是隐藏到托盘（close 已被拦截，正常不会走到这里）；不监听此事件的话 Electron 默认关窗即退出
app.on('window-all-closed', () => {});
// 真正退出（托盘菜单「退出」、macOS Cmd+Q）：放行 close 拦截，由 will-quit 回收 server
app.on('before-quit', () => {
  isQuitting = true;
});
// macOS：关窗（隐藏到托盘）后点击 Dock 图标重新打开
app.on('activate', showMainWindow);
app.on('will-quit', stopServer);

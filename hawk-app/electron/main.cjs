// hawk-app Electron 主进程：窗口管理、拉起/回收 hawk-server、token 注入、库选择、白名单 IPC。
// 业务数据一律走 REST，不经 IPC（见 docs/architecture.md、docs/hawk-app.md）。
const { app, BrowserWindow, dialog, ipcMain, shell } = require('electron');
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
/** 当前素材库根目录（show-in-finder 的路径守卫要用） */
let libraryRoot = null;

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
    // 无边框窗口：标题栏由前端自绘（Eagle 式通栏），窗口控制走 hawk:win-* IPC
    frame: false,
    // 开发态 / Linux 的窗口图标；打包后各平台图标由 electron-builder 嵌入
    icon: path.join(__dirname, '..', 'build', 'icon.png'),
    webPreferences: {
      preload: path.join(__dirname, 'preload.cjs'),
    },
  });
  mainWindow.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
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

app.on('window-all-closed', () => app.quit());
app.on('will-quit', stopServer);

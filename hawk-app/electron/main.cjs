// hawk-app Electron 主进程：窗口管理（关窗隐藏到托盘）、系统托盘、单实例、拉起/回收 hawk-server、token 注入、库选择、白名单 IPC。
// 业务数据一律走 REST，不经 IPC（见 docs/architecture.md、docs/frontend/hawk-app.md）。
const { app, BrowserWindow, dialog, ipcMain, shell, Menu, Tray, nativeImage, clipboard } = require('electron');
const { spawn } = require('node:child_process');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');
const fs = require('node:fs');
const crypto = require('node:crypto');

const isDev = !app.isPackaged;
const CONFIG_FILE = () => path.join(app.getPath('userData'), 'hawk-app.json');

/** @type {{ child: import('child_process').ChildProcess, address: string, token: string, ready: Promise<void>, markStopped(): void } | null} */
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
// （第二个实例会拉起第二套 hawk-server 进程争用同一素材库，引发索引与文件监听竞争）
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
    // 开发态：直接运行 Rust 后端二进制（release 优先；后端开发迭代的 debug 构建亦可）
    const exe = process.platform === 'win32' ? 'hawk-server.exe' : 'hawk-server';
    const RUST_TARGET = { 'win32-x64': 'x86_64-pc-windows-msvc', 'darwin-arm64': 'aarch64-apple-darwin', 'darwin-x64': 'x86_64-apple-darwin', 'linux-x64': 'x86_64-unknown-linux-gnu' }[`${process.platform}-${process.arch}`];
    const targetDir = path.join(__dirname, '..', '..', 'hawk-server-rs', 'target');
    // 兼容两种 cargo 产物位置：本机直建 target/release 与 --target 交叉建 target/<triple>/release
    const candidates = [
      ...(RUST_TARGET ? [path.join(targetDir, RUST_TARGET, 'release')] : []),
      path.join(targetDir, 'release'),
      path.join(targetDir, 'debug'),
    ];
    for (const dir of candidates) {
      const bin = path.join(dir, exe);
      if (fs.existsSync(bin)) {
        return { command: bin, args: [] };
      }
    }
    throw new Error('未找到 hawk-server-rs 构建产物，请先 cargo build --release（hawk-server-rs/）');
  }
  // 打包态：extraResources 携带的 Rust 二进制（cargo build --release，见 scripts/build-server.mjs）
  const bin = process.platform === 'win32' ? 'hawk-server.exe' : 'hawk-server';
  return { command: path.join(process.resourcesPath, 'hawk-server', bin), args: [] };
}

/** 预选一个空闲环回端口：server 绑定它，token 由本进程生成——端口与 token 都不再需要子进程回传 */
function probeFreePort() {
  return new Promise((resolve, reject) => {
    const probe = net.createServer();
    probe.once('error', reject);
    probe.listen(0, '127.0.0.1', () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });
}

/** LAN web 查看托管的前端产物：与 loadMainPage 同一目录（dev 与打包形态路径一致）。
 *  打包态 web/dist 经 asarUnpack 落在 app.asar.unpacked 物理路径——Kestrel 读不到 asar 内部 */
function webDistDir() {
  const dist = path.join(__dirname, '..', 'web', 'dist');
  if (!isDev) {
    const unpacked = dist.replace(`${path.sep}app.asar${path.sep}`, `${path.sep}app.asar.unpacked${path.sep}`);
    if (fs.existsSync(unpacked)) {
      return unpacked;
    }
  }
  return dist;
}

/**
 * 拉起 hawk-server：监听端口、初始索引后台构建（正规 HTTP 握手，无 stdout 私有协议）。
 * 页面已先行加载并显示应用内启动屏，此处不再等待就绪——进度/就绪/错误经 IPC 事件推送：
 *   hawk:server-progress（starting 阶段进度）→ hawk:server-started（就绪，含地址与 token）→ hawk:server-error（失败原因）。
 * spawn 失败、异常退出（stopServer 除外）、60s 超时就 hawk:server-error；返回句柄含 ready Promise（save-lan-settings 回滚要用）。
 */
function startServer(libPath, address, token) {
  const { command, args } = resolveServerCommand();
  // 闭包级标志：有意停止（换库/应用设置重启）时抑制 exit 广播——旧子进程终止可能晚于
  // 新 server 的拉起，全局标志会被新一轮复位，造成误报异常退出
  let intentionalExit = false;
  const child = spawn(
    command,
    [...args, '--library', libPath, '--port', String(new URL(address).port), '--web-dist', webDistDir()],
    {
      env: { ...process.env, HAWK_TOKEN: token },
      stdio: ['ignore', 'ignore', 'pipe'], // stdout 不再承担协议，只看 stderr 报错
      // GUI 进程拉起控制台子进程：不隐藏会在 Windows 上弹出黑窗口
      windowsHide: true,
    },
  );

  let settleReady;
  const ready = new Promise((resolve, reject) => {
    settleReady = { resolve, reject };
  });
  let stderrTail = '';
  let poll = 0;
  let watchdog = 0;
  let lastProgressAt = Date.now();
  /** 失败统一出口：通知渲染进程 + reject ready（一次性） */
  const fail = (message) => {
    clearInterval(poll);
    clearInterval(watchdog);
    mainWindow?.webContents.send('hawk:server-error', { message });
    settleReady.reject(new Error(message));
  };

  // 留 stderr 尾部用于报错；开发态同时转发到终端
  child.stderr.on('data', (chunk) => {
    stderrTail = (stderrTail + chunk.toString()).slice(-4000);
    if (isDev) {
      process.stderr.write(chunk);
    }
  });
  child.on('error', (error) => fail(`hawk-server 启动失败: ${error.message}`));
  child.on('exit', (code) => {
    if (!intentionalExit) {
      fail(`hawk-server 异常退出（退出码 ${code}）${stderrTail.trim() ? `\n${stderrTail.trim()}` : ''}`);
    }
  });

  poll = setInterval(async () => {
    try {
      const res = await fetch(`${address}/api/v1/app/startup`, {
        headers: { authorization: `Bearer ${token}` },
      });
      if (!res.ok) {
        return; // 服务已监听但未到可查询状态，继续轮询
      }
      // 可应答即视为活着（慢任务容忍：缓存重建/TOML 全量解析可达数分钟），重置停滞计时
      lastProgressAt = Date.now();
      const body = await res.json();
      const state = body.data;
      if (state.status === 'starting') {
        mainWindow?.webContents.send('hawk:server-progress', {
          phase: state.phase || 'scan',
          processed: state.processed || 0,
          total: state.total || 0,
        });
        return;
      }
      clearInterval(poll);
      clearInterval(watchdog);
      if (state.status === 'ready') {
        mainWindow?.webContents.send('hawk:server-started', { address, token });
        settleReady.resolve();
      } else {
        fail(state.message || 'hawk-server 初始索引构建失败');
      }
    } catch {
      // 连接拒绝：server 尚未监听，继续轮询
    }
  }, 200);
  // 停滞看门狗：只防「HTTP 都无响应」的真卡死（Kestrel 线程池耗尽/进程 hang）；
  // 能应答 startup 就算慢也不超时。进程崩溃由 exit 事件单独上报
  watchdog = setInterval(() => {
    if (Date.now() - lastProgressAt >= 120_000) {
      fail('hawk-server 启动无响应，疑似卡死');
    }
  }, 1000);

  return { child, address, token, ready, markStopped: () => (intentionalExit = true) };
}

function stopServer() {
  if (server) {
    server.markStopped();
    if (!server.child.killed) {
      server.child.kill();
    }
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

function loadMainPage(conn) {
  const hash = conn ? `api=${encodeURIComponent(conn.address)}&token=${conn.token}` : '';
  if (isDev) {
    mainWindow.loadURL(`http://localhost:5173/${hash ? `#${hash}` : ''}`);
  } else {
    mainWindow.loadFile(path.join(__dirname, '..', 'web', 'dist', 'index.html'), { hash });
  }
}

function createWindow() {
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
      preload: path.join(__dirname, 'preload.cjs'),
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

async function switchLibrary(libPath, address, token) {
  // 前端立刻切启动屏：旧 server 已停、新 server 未 ready 的窗口期，
  // 主界面所有 API 已失效（假死），不能在 ready 后才切（hawk:server-restarting）
  mainWindow?.webContents.send('hawk:server-restarting');
  stopServer();
  // 记住当前库并维护历史（最近使用在前、去重、上限 10）：换库下拉经 hawk:list-libraries 直达
  const history = [libPath, ...(readConfig().libraryHistory ?? []).filter((p) => p !== libPath)].slice(0, 10);
  writeConfig({ libraryPath: libPath, libraryHistory: history });
  libraryRoot = libPath;
  return startServer(libPath, address, token);
}

/** 拉起指定素材库的 server（选新目录/历史库/冷启动共用）：端口/token 即选即生成 */
async function openLibraryAt(libPath) {
  const token = crypto.randomBytes(32).toString('hex');
  const port = await probeFreePort();
  return switchLibrary(libPath, `http://127.0.0.1:${port}`, token);
}

/** 历史库列表（最近使用在前，含目录存在性；当前库由 libraryRoot 标记） */
function listLibraries() {
  const history = readConfig().libraryHistory ?? [];
  return {
    current: libraryRoot,
    libraries: history
      .filter((p) => typeof p === 'string')
      .map((p) => ({ path: p, name: path.basename(p), exists: fs.existsSync(p) })),
  };
}

// ---------- 局域网 web 查看（[web] 段按库隔离，存于 .hawk/config.toml） ----------

const WEB_DEFAULTS = { enabled: false, port: 27372, token: '' };

function libraryConfigFile() {
  return path.join(libraryRoot, '.hawk', 'config.toml');
}

/** 文本级读取 [web] 段（保留文件其余内容不解析；TOML 由 server 权威解析） */
function readWebSection(file) {
  const out = { ...WEB_DEFAULTS };
  try {
    let inWeb = false;
    for (const line of fs.readFileSync(file, 'utf8').split(/\r?\n/)) {
      const section = line.match(/^\s*\[([^\]]+)\]\s*$/);
      if (section) {
        inWeb = section[1] === 'web';
        continue;
      }
      if (!inWeb) continue;
      const kv = line.match(/^\s*([A-Za-z_]+)\s*=\s*(.+?)\s*$/);
      if (!kv) continue;
      const key = kv[1].toLowerCase();
      const quoted = kv[2].match(/^"(.*)"$/) ?? kv[2].match(/^'(.*)'$/);
      const value = quoted ? quoted[1] : kv[2].replace(/\s+#.*$/, '');
      if (key === 'enabled') out.enabled = value === 'true';
      else if (key === 'port') out.port = Number.parseInt(value, 10) || WEB_DEFAULTS.port;
      else if (key === 'token') out.token = value;
    }
  } catch { /* 文件不存在等,用默认 */ }
  if (!(out.port > 0 && out.port <= 65535)) out.port = WEB_DEFAULTS.port;
  return out;
}

/** 文本级写回 [web] 段：整段替换、其余段原样保留，新段追加文件末尾 */
function writeWebSection(file, web) {
  const lines = fs.existsSync(file) ? fs.readFileSync(file, 'utf8').split(/\r?\n/) : [];
  const kept = [];
  let inWeb = false;
  for (const line of lines) {
    const section = line.match(/^\s*\[([^\]]+)\]\s*$/);
    if (section) {
      inWeb = section[1] === 'web'; // 丢弃旧 [web] 段头,段体随 inWeb 跳过
      if (!inWeb) kept.push(line);
      continue;
    }
    if (!inWeb) kept.push(line);
  }
  while (kept.length && kept[kept.length - 1].trim() === '') kept.pop();
  const token = String(web.token).replace(/["\\]/g, '');
  kept.push('[web]', `enabled = ${web.enabled ? 'true' : 'false'}`, `port = ${web.port}`, `token = "${token}"`, '');
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, kept.join('\n'));
}

function lanAddresses() {
  const addresses = [];
  for (const list of Object.values(os.networkInterfaces())) {
    for (const item of list ?? []) {
      // Node 18+ family 为数字 4/6（旧版本为字符串 'IPv4'/'IPv6'），两种都兼容
      const isIpv4 = item.family === 4 || item.family === 'IPv4';
      if (isIpv4 && !item.internal) addresses.push(item.address);
    }
  }
  return addresses;
}

// ---------- 白名单 IPC ----------

// 真正退出应用（启动错误屏的「退出 hawk」按钮）：置放行标志后退出，回收 server
ipcMain.handle('hawk:quit-app', () => {
  isQuitting = true;
  app.quit();
});

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
    // 端口/token 即选即生成；页面切应用内启动屏，就绪经 hawk:server-started 通知
    server = await openLibraryAt(selected);
    return true;
  } catch (error) {
    // 失败时留在引导页并给出可见错误，而不是让 IPC 静默 reject
    dialog.showErrorBox('hawk-server 启动失败', String(error && error.message ? error.message : error));
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
    server = await openLibraryAt(libPath);
    return true;
  } catch (error) {
    dialog.showErrorBox('hawk-server 启动失败', String(error && error.message ? error.message : error));
    return false;
  }
});

ipcMain.handle('hawk:get-lan-settings', () => ({
  ...readWebSection(libraryConfigFile()),
  addresses: lanAddresses(),
}));

ipcMain.handle('hawk:save-lan-settings', async (_event, web) => {
  const norm = {
    enabled: !!web?.enabled,
    port: Number.parseInt(web?.port, 10) || WEB_DEFAULTS.port,
    token: String(web?.token ?? '').trim(),
  };
  if (norm.enabled && !norm.token) {
    return { ok: false, error: '启用局域网查看需要填写访问 token' };
  }

  const file = libraryConfigFile();
  const backup = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : null;
  writeWebSection(file, norm);
  // 端口/绑定的生效需重启监听。await ready 以支持失败回滚（spawn 同步失败由 select-library 路径的
  // 系统弹窗兜底；此处失败信号来自 ready reject / hawk:server-error 事件）
  try {
    server = await openLibraryAt(libraryRoot);
    await server.ready;
  } catch (error) {
    if (backup === null) fs.rmSync(file, { force: true });
    else fs.writeFileSync(file, backup);
    try {
      stopServer();
      server = await openLibraryAt(libraryRoot);
      await server.ready;
    } catch {
      // 尽力恢复;仍失败则保持现状由用户重启应用
    }
    const message = String(error && error.message ? error.message : error);
    dialog.showErrorBox('应用设置失败', message);
    return { ok: false, error: message };
  }
  // 无需重载页面：hawk:server-started 事件驱动渲染进程原地换地址重启数据
  return { ok: true };
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

/** 库内相对路径 → 绝对路径（含越界守卫），非法路径返回 null */
function resolveLibraryPath(relPath) {
  if (typeof relPath !== 'string' || relPath.includes('..') || !libraryRoot) {
    return null;
  }
  const abs = path.join(libraryRoot, ...relPath.split('/'));
  return path.resolve(abs).startsWith(path.resolve(libraryRoot)) ? abs : null;
}

// ---------- 生命周期 ----------

app.whenReady().then(async () => {
  createWindow();
  createTray();

  const libPath = readConfig().libraryPath;
  if (!libPath || !fs.existsSync(libPath)) {
    loadMainPage(); // 素材库未配置或已失效：进应用内引导页（无连接参数）
    return;
  }

  // 端口/token 先生成、页面立即加载并显示应用内启动屏，server 后台拉起——
  // 窗口内容单页生命周期，无 loading→主界面二次导航，杜绝切换白屏
  try {
    server = await openLibraryAt(libPath);
    loadMainPage({ address: server.address, token: server.token });
  } catch (error) {
    dialog.showErrorBox('hawk-server 启动失败', String(error));
    app.quit();
    return;
  }
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

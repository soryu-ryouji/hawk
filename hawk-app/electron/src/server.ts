// hawk-daemon 进程管理：二进制解析、空闲端口预选、拉起（先监听后索引的就绪轮询）/回收、换库。
// 业务数据一律走 REST，握手全程正规 HTTP（无 stdout 私有协议）。
import { app, dialog } from 'electron';
import { spawn, type ChildProcess } from 'node:child_process';
import net from 'node:net';
import path from 'node:path';
import fs from 'node:fs';
import crypto from 'node:crypto';
import { APP_DIR, DAEMON_LOG_FILE, REMOTE_CONFIG_FILE } from './paths';
import { getMainWindow } from './window';
import { API_PORT_RANGE, getApiPort, getLanTarget, getLibraryRoot, getRemoteTarget, readConfig, setApiPort, setLibraryRoot, writeConfig } from './app-config';
import { IPC, PORT_IN_USE, type ServerConn, type ServerFailure } from './ipc-contract';
import { onServerReady, onServerStopped, restoreLanSession, restoreRemoteSession } from './remote';

const isDev = !app.isPackaged;

/** daemon 端口占用退出码（与 hawk-daemon/src/bootstrap.rs 的 bind_failed 对齐） */
const PORT_IN_USE_EXIT_CODE = 4;

export interface ServerHandle {
  child: ChildProcess;
  address: string;
  token: string;
  markStopped(): void;
}

let server: ServerHandle | null = null;
/** 已就绪 server 的连接参数（ready 时写入，停服/换库时清空）；页面加载晚于就绪时事件会丢，
 *  渲染进程经 hawk:server-conn / remoteGetConnection 主动拉取兜底 */
let startedConn: ServerConn | null = null;

/** 最近一次启动/运行失败（拉取兜底：daemon 端口占用会在页面订阅 IPC 之前就退出，
 *  只靠推送会丢事件——渲染进程挂载时经 getServerFailure 主动拉一次）。
 *  每次新拉起尝试开始时清空，避免展示上次的旧错误 */
let lastFailure: ServerFailure | null = null;

export function getServerFailure(): ServerFailure | null {
  return lastFailure;
}

export function getStartedConn(): ServerConn | null {
  return startedConn;
}

function resolveServerCommand(): { command: string; args: string[] } {
  if (process.env.HAWK_DAEMON_EXE) {
    return { command: process.env.HAWK_DAEMON_EXE, args: [] };
  }
  if (isDev) {
    // 开发态：直接运行 Rust 后端二进制（release 优先；后端开发迭代的 debug 构建亦可）
    const exe = process.platform === 'win32' ? 'hawk-daemon.exe' : 'hawk-daemon';
    const RUST_TARGET: Record<string, string> = {
      'win32-x64': 'x86_64-pc-windows-msvc',
      'darwin-arm64': 'aarch64-apple-darwin',
      'darwin-x64': 'x86_64-apple-darwin',
      'linux-x64': 'x86_64-unknown-linux-gnu',
    };
    const targetDir = path.join(APP_DIR, '..', 'hawk-daemon', 'target');
    // 兼容两种 cargo 产物位置：本机直建 target/release 与 --target 交叉建 target/<triple>/release。
    // 多个产物并存时按 mtime 取最新（与 scripts/gen-types.mjs 同策略）——固定优先级会
    // 在交叉产物过期时静默用旧 daemon，与 gen-types 选中的二进制版本不一致
    const candidates = [
      ...(RUST_TARGET[`${process.platform}-${process.arch}`] ? [path.join(targetDir, RUST_TARGET[`${process.platform}-${process.arch}`], 'release')] : []),
      path.join(targetDir, 'release'),
      path.join(targetDir, 'debug'),
    ];
    const existing = candidates
      .map((dir) => path.join(dir, exe))
      .filter((bin) => fs.existsSync(bin))
      .sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs);
    if (existing.length > 0) {
      return { command: existing[0], args: [] };
    }
    throw new Error('未找到 hawk-daemon 构建产物，请先 cargo build --release（hawk-daemon/）');
  }
  // 打包态：extraResources 携带的 Rust 二进制（cargo build --release，见 scripts/build-server.mjs）
  const bin = process.platform === 'win32' ? 'hawk-daemon.exe' : 'hawk-daemon';
  return { command: path.join(process.resourcesPath, 'hawk-daemon', bin), args: [] };
}

/** 本机 daemon 的监听地址：端口取自用户配置（config.toml apiPort，默认 27371）。
 *  端口所有权只在本文件：daemon 严格绑定这个端口，绑不上就报错退出（不静默漂移），
 *  浏览器插件等生态客户端才能按固定段探活 */
function apiAddress(): string {
  return `http://127.0.0.1:${getApiPort()}`;
}

/** 端口能否绑定（环回）：换端口前的预检，真正的裁决在 daemon 的严格绑定 */
function tryListen(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const probe = net.createServer();
    probe.once('error', () => resolve(false));
    probe.listen(port, '127.0.0.1', () => probe.close(() => resolve(true)));
  });
}

/** 在候选段内挑第一个空闲端口；全占返回 null */
async function findFreePortInRange(): Promise<number | null> {
  for (let port = API_PORT_RANGE[0]; port <= API_PORT_RANGE[1]; port += 1) {
    if (await tryListen(port)) {
      return port;
    }
  }
  return null;
}

/** LAN web 查看托管的前端产物：与 loadMainPage 同一目录（dev 与打包形态路径一致）。
 *  打包态 web/dist 经 asarUnpack 落在 app.asar.unpacked 物理路径——后端读不到 asar 内部 */
function webDistDir(): string {
  const dist = path.join(APP_DIR, 'web', 'dist');
  if (!isDev) {
    const unpacked = dist.replace(`${path.sep}app.asar${path.sep}`, `${path.sep}app.asar.unpacked${path.sep}`);
    if (fs.existsSync(unpacked)) {
      return unpacked;
    }
  }
  return dist;
}

/**
 * 就绪通知（每次 server 就绪一次）：上次浏览的是远端库时先尝试恢复会话，再通知渲染进程。
 * 恢复成功时 connection-changed 已把 API 指向会话代理，渲染进程首个注水即远端库（不先闪本地库）；
 * 失败/无记录则按本地库通知。恢复期间停服/换库时用 startedConn 身份比对废弃本次通知。
 */
async function notifyReady(): Promise<void> {
  const conn = startedConn;
  if (!conn) {
    return;
  }
  if (getRemoteTarget()) {
    // 启动屏先切文案：恢复可能耗时（信令接入/拨号），期间没有索引进度可报，
    // 不能停在上一轮的扫描文案上
    getMainWindow()?.webContents.send(IPC.serverProgress, { phase: 'restore-remote', processed: 0, total: 0 });
    if (await restoreRemoteSession()) {
      return;
    }
    if (startedConn !== conn) {
      return;
    }
  } else if (getLanTarget()) {
    // 局域网直连不依赖服务器：上一轮浏览另一台 hawk-app 时优先恢复同一目标。
    getMainWindow()?.webContents.send(IPC.serverProgress, { phase: 'restore-lan', processed: 0, total: 0 });
    if (await restoreLanSession()) {
      return;
    }
    if (startedConn !== conn) {
      return;
    }
  }
  getMainWindow()?.webContents.send(IPC.serverStarted, conn);
}

/**
 * 拉起 hawk-daemon：监听端口、初始索引后台构建（正规 HTTP 握手，无 stdout 私有协议）。
 * libPath 为 null 即远程查看模式（daemon 无库形态：仅远程管理面与会话代理可用）。
 * 页面已先行加载并显示应用内启动屏，此处不再等待就绪——进度/就绪/错误经 IPC 事件推送：
 *   hawk:server-progress（starting 阶段进度）→ hawk:server-started（就绪，含地址与 token）→ hawk:server-error（失败原因）。
 * spawn 失败、异常退出（stopServer 除外）、停滞 120s 超时就 hawk:server-error。
 */
function startServer(libPath: string | null, address: string, token: string): ServerHandle {
  lastFailure = null; // 新的一次拉起：清掉上一轮失败（页面挂载时的拉取兜底不应看到旧错）
  const { command, args } = resolveServerCommand();
  // 全局缓存父目录（设置面板「存储」配置；未配置用系统默认）
  const cacheParent = readConfig().cacheParent;
  const spawnArgs = [
    ...args,
    // 无库（远程查看模式）不传 --library：远程配置与设备身份在应用级 remote.toml
    ...(libPath ? ['--library', libPath] : []),
    '--port',
    String(new URL(address).port),
    '--web-dist',
    webDistDir(),
    ...(cacheParent ? ['--cache-parent', cacheParent] : []),
    // 远程接入配置跨素材库共享：固定指向应用级 remote.toml（身份目录由其同级派生）
    '--remote-config',
    REMOTE_CONFIG_FILE,
  ];
  // 闭包级标志：有意停止（换库/应用设置重启）时抑制 exit 广播——旧子进程终止可能晚于
  // 新 server 的拉起，全局标志会被新一轮复位，造成误报异常退出
  let intentionalExit = false;
  const child = spawn(command, spawnArgs, {
    env: { ...process.env, HAWK_TOKEN: token },
    stdio: ['ignore', 'pipe', 'pipe'], // stdout 落日志文件（tracing info 级是远程排障主力），stderr 报错
    // GUI 进程拉起控制台子进程：不隐藏会在 Windows 上弹出黑窗口
    windowsHide: true,
  });
  // daemon 日志落盘（每次拉起截断重写，只看当次会话）；日志失败不阻断启动
  let logStream: fs.WriteStream | null = null;
  try {
    fs.mkdirSync(path.dirname(DAEMON_LOG_FILE), { recursive: true });
    logStream = fs.createWriteStream(DAEMON_LOG_FILE, { flags: 'w' });
    logStream.on('error', () => {});
  } catch {
    logStream = null;
  }
  child.stdout?.on('data', (chunk: Buffer) => {
    logStream?.write(chunk);
    if (isDev) {
      process.stdout.write(chunk);
    }
  });

  let stderrTail = '';
  let poll: ReturnType<typeof setInterval> | undefined;
  let watchdog: ReturnType<typeof setInterval> | undefined;
  let lastProgressAt = Date.now();
  /** 失败统一出口：通知渲染进程（一次性）；带 code 时前端据此分流（端口冲突 → 换端口界面） */
  const fail = (message: string, failure?: Omit<ServerFailure, 'message'>): void => {
    clearInterval(poll);
    clearInterval(watchdog);
    const payload: ServerFailure = { message, ...failure };
    lastFailure = payload;
    getMainWindow()?.webContents.send(IPC.serverError, payload);
  };

  // 留 stderr 尾部用于报错；开发态同时转发到终端
  child.stderr?.on('data', (chunk: Buffer) => {
    stderrTail = (stderrTail + chunk.toString()).slice(-4000);
    if (isDev) {
      process.stderr.write(chunk);
    }
  });
  child.on('error', (error) => {
    const missing = (error as NodeJS.ErrnoException).code === 'ENOENT';
    fail(`hawk-daemon 启动失败: ${error.message}${missing ? '（安装包缺少 hawk-daemon，请重新安装完整版本）' : ''}`);
  });
  child.on('exit', (code) => {
    logStream?.end();
    if (!intentionalExit) {
      // 端口占用（daemon 退出码 4 / 固定文案）：交给渲染进程的端口冲突界面处理，
      // 不要甩 stderr 给用户也不自动换端口（端口的唯一所有者是壳层，见上）
      const port = getApiPort();
      if (code === PORT_IN_USE_EXIT_CODE || stderrTail.includes(PORT_IN_USE)) {
        fail(`本地端口 ${port} 已被占用，本地服务无法启动`, { code: PORT_IN_USE, port });
        return;
      }
      // 旧 daemon 不认识新参数：提示版本不匹配，而不是只甩 clap 用法
      const stale = stderrTail.includes("unexpected argument '--remote-config'")
        ? '\n\nhawk-daemon 版本过旧或安装包不完整：请完全退出 hawk 后重新安装，或重新构建 hawk-daemon。'
        : '';
      fail(`hawk-daemon 异常退出（退出码 ${code}）${stderrTail.trim() ? `\n${stderrTail.trim()}` : ''}${stale}`);
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
      const body = (await res.json()) as { data: { status: string; phase?: string; processed?: number; total?: number; message?: string } };
      const state = body.data;
      if (state.status === 'starting') {
        getMainWindow()?.webContents.send(IPC.serverProgress, {
          phase: state.phase || 'scan',
          processed: state.processed || 0,
          total: state.total || 0,
        });
        return;
      }
      clearInterval(poll);
      clearInterval(watchdog);
      if (state.status === 'ready') {
        lastFailure = null;
        startedConn = { address, token };
        // remote SSE 订阅重建（remote.session 断线回切依赖；见 remote.ts）
        onServerReady();
        void notifyReady();
      } else {
        fail(state.message || 'hawk-daemon 初始索引构建失败');
      }
    } catch {
      // 连接拒绝：server 尚未监听，继续轮询
    }
  }, 200);
  // 停滞看门狗：只防「HTTP 都无响应」的真卡死（线程池耗尽/进程 hang）；
  // 能应答 startup 就算慢也不超时。进程崩溃由 exit 事件单独上报
  watchdog = setInterval(() => {
    if (Date.now() - lastProgressAt >= 120_000) {
      fail('hawk-daemon 启动无响应，疑似卡死');
    }
  }, 1000);

  return { child, address, token, markStopped: () => (intentionalExit = true) };
}

export function stopServer(): void {
  startedConn = null;
  onServerStopped();
  if (server) {
    server.markStopped();
    if (!server.child.killed) {
      server.child.kill();
    }
  }
  server = null;
}

// ---------- 素材库选择 ----------

export async function pickLibrary(): Promise<string | null> {
  const win = getMainWindow();
  if (!win) {
    return null;
  }
  const result = await dialog.showOpenDialog(win, {
    title: '选择素材库目录',
    properties: ['openDirectory', 'createDirectory'],
  });
  return result.canceled ? null : result.filePaths[0];
}

async function switchLibrary(libPath: string, address: string, token: string): Promise<ServerHandle> {
  // 前端立刻切启动屏：旧 server 已停、新 server 未 ready 的窗口期，
  // 主界面所有 API 已失效（假死），不能在 ready 后才切（hawk:server-restarting）
  getMainWindow()?.webContents.send(IPC.serverRestarting);
  startedConn = null;
  stopServer();
  // 记住当前库并维护历史（最近使用在前、去重、上限 10）：换库下拉经 hawk:list-libraries 直达
  const history = [libPath, ...(readConfig().libraryHistory ?? []).filter((p) => p !== libPath)].slice(0, 10);
  writeConfig({ libraryPath: libPath, libraryHistory: history, remoteViewer: false });
  setLibraryRoot(libPath);
  return startServer(libPath, address, token);
}

/** 拉起指定素材库的 server（选新目录/历史库/冷启动共用）：端口取自用户配置，token 即选即生成 */
export async function openLibraryAt(libPath: string): Promise<ServerHandle> {
  const token = crypto.randomBytes(32).toString('hex');
  server = await switchLibrary(libPath, apiAddress(), token);
  return server;
}

/** 远程查看模式：拉起无库 daemon（仅远程管理面与会话代理），不创建任何本地目录。
 *  就绪后前端进远程查看屏（配置服务端/选设备）；冷启动模式记忆为 remoteViewer */
export async function startRemoteViewer(): Promise<ServerHandle> {
  const token = crypto.randomBytes(32).toString('hex');
  getMainWindow()?.webContents.send(IPC.serverRestarting);
  startedConn = null;
  stopServer();
  writeConfig({ remoteViewer: true });
  setLibraryRoot(null);
  server = startServer(null, apiAddress(), token);
  return server;
}

/** 按当前模式重启 daemon（本地库 → 同库；远程查看 → 无库形态）。设置面板与端口重试共用 */
export async function restartCurrent(): Promise<void> {
  const libRoot = getLibraryRoot();
  if (libRoot) {
    await openLibraryAt(libRoot);
  } else {
    await startRemoteViewer();
  }
}

/** 端口冲突后的重试：auto 在候选段内挑空闲端口，port 用用户手输（任意 1024–65535）；
 *  写入配置后按当前模式重启。返回错误文案或 null（null = 已重启，就绪经 onServerStarted 通知） */
export async function retryServerPort(request: { mode: 'auto' } | { port: number }): Promise<string | null> {
  let port: number;
  if ('mode' in request) {
    const free = await findFreePortInRange();
    if (free === null) {
      return `端口 ${API_PORT_RANGE[0]}–${API_PORT_RANGE[1]} 都已占用，请手动指定其他端口`;
    }
    port = free;
  } else {
    port = request.port;
    if (!Number.isInteger(port) || port < 1024 || port > 65535) {
      return '端口须为 1024–65535 之间的整数';
    }
    if (!(await tryListen(port))) {
      return `端口 ${port} 已被占用，换一个试试`;
    }
  }
  setApiPort(port);
  await restartCurrent();
  return null;
}

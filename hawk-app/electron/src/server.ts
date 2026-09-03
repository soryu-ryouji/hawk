// hawk-daemon 进程管理：二进制解析、空闲端口预选、拉起（先监听后索引的就绪轮询）/回收、换库。
// 业务数据一律走 REST，握手全程正规 HTTP（无 stdout 私有协议）。
import { app, dialog } from 'electron';
import { spawn, type ChildProcess } from 'node:child_process';
import net from 'node:net';
import path from 'node:path';
import fs from 'node:fs';
import crypto from 'node:crypto';
import { APP_DIR } from './paths';
import { getMainWindow } from './window';
import { readConfig, setLibraryRoot, writeConfig } from './app-config';
import { IPC } from './ipc-contract';

const isDev = !app.isPackaged;

export interface ServerHandle {
  child: ChildProcess;
  address: string;
  token: string;
  markStopped(): void;
}

let server: ServerHandle | null = null;

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
    // 兼容两种 cargo 产物位置：本机直建 target/release 与 --target 交叉建 target/<triple>/release
    const candidates = [
      ...(RUST_TARGET[`${process.platform}-${process.arch}`]
        ? [path.join(targetDir, RUST_TARGET[`${process.platform}-${process.arch}`], 'release')]
        : []),
      path.join(targetDir, 'release'),
      path.join(targetDir, 'debug'),
    ];
    for (const dir of candidates) {
      const bin = path.join(dir, exe);
      if (fs.existsSync(bin)) {
        return { command: bin, args: [] };
      }
    }
    throw new Error('未找到 hawk-daemon 构建产物，请先 cargo build --release（hawk-daemon/）');
  }
  // 打包态：extraResources 携带的 Rust 二进制（cargo build --release，见 scripts/build-server.mjs）
  const bin = process.platform === 'win32' ? 'hawk-daemon.exe' : 'hawk-daemon';
  return { command: path.join(process.resourcesPath, 'hawk-daemon', bin), args: [] };
}

/** 预选一个空闲环回端口：server 绑定它，token 由本进程生成——端口与 token 都不再需要子进程回传 */
function probeFreePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = net.createServer();
    probe.once('error', reject);
    probe.listen(0, '127.0.0.1', () => {
      const address = probe.address();
      if (typeof address !== 'object' || address === null) {
        probe.close(() => reject(new Error('预选端口失败')));
        return;
      }
      probe.close(() => resolve(address.port));
    });
  });
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
 * 拉起 hawk-daemon：监听端口、初始索引后台构建（正规 HTTP 握手，无 stdout 私有协议）。
 * 页面已先行加载并显示应用内启动屏，此处不再等待就绪——进度/就绪/错误经 IPC 事件推送：
 *   hawk:server-progress（starting 阶段进度）→ hawk:server-started（就绪，含地址与 token）→ hawk:server-error（失败原因）。
 * spawn 失败、异常退出（stopServer 除外）、停滞 120s 超时就 hawk:server-error。
 */
function startServer(libPath: string, address: string, token: string): ServerHandle {
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

  let stderrTail = '';
  let poll: ReturnType<typeof setInterval> | undefined;
  let watchdog: ReturnType<typeof setInterval> | undefined;
  let lastProgressAt = Date.now();
  /** 失败统一出口：通知渲染进程（一次性） */
  const fail = (message: string): void => {
    clearInterval(poll);
    clearInterval(watchdog);
    getMainWindow()?.webContents.send(IPC.serverError, { message });
  };

  // 留 stderr 尾部用于报错；开发态同时转发到终端
  child.stderr?.on('data', (chunk: Buffer) => {
    stderrTail = (stderrTail + chunk.toString()).slice(-4000);
    if (isDev) {
      process.stderr.write(chunk);
    }
  });
  child.on('error', (error) => fail(`hawk-daemon 启动失败: ${error.message}`));
  child.on('exit', (code) => {
    if (!intentionalExit) {
      fail(`hawk-daemon 异常退出（退出码 ${code}）${stderrTail.trim() ? `\n${stderrTail.trim()}` : ''}`);
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
        getMainWindow()?.webContents.send(IPC.serverStarted, { address, token });
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
  stopServer();
  // 记住当前库并维护历史（最近使用在前、去重、上限 10）：换库下拉经 hawk:list-libraries 直达
  const history = [libPath, ...(readConfig().libraryHistory ?? []).filter((p) => p !== libPath)].slice(0, 10);
  writeConfig({ libraryPath: libPath, libraryHistory: history });
  setLibraryRoot(libPath);
  return startServer(libPath, address, token);
}

/** 拉起指定素材库的 server（选新目录/历史库/冷启动共用）：端口/token 即选即生成 */
export async function openLibraryAt(libPath: string): Promise<ServerHandle> {
  const token = crypto.randomBytes(32).toString('hex');
  const port = await probeFreePort();
  server = await switchLibrary(libPath, `http://127.0.0.1:${port}`, token);
  return server;
}

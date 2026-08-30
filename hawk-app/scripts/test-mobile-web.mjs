// 移动端网页冒烟测试：npm run test:mobile
// 覆盖局域网手机浏览器全链路（无 hawkShell 的轮询启动路径，桌面 IPC 路径之外的另一半）：
//   生成临时素材库（含全景/竖图，顺便校验齐行布局不横向溢出）→ 拉起 hawk-server 托管 web/dist
//   → 以无 preload 的 Electron 窗口模拟手机浏览器（390×844）→ 断言：
//   启动屏出现 → 轮询就绪进入主界面（卡在启动屏即失败，正是 2026-08 回归的故障模式）
//   → 网格渲染且无横向溢出 → 窄屏顶栏：排序/筛选收进溢出菜单、搜索退化为按钮并可点开浮层
//   → 点按卡片开预览且中央图在视口内。
// 产物（截图/临时库）放在 .tmp/mobile-smoke/，成功即清理，失败保留供排查。
import { spawn, spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import net from 'node:net';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { deflateSync } from 'node:zlib';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(import.meta.url);
const tmpDir = path.join(root, '.tmp', 'mobile-smoke');
const shotsDir = path.join(tmpDir, 'shots');

// ---------- 最小 PNG 生成（任意尺寸纯色图，免依赖） ----------
function crc32(buf) {
  let table = crc32.table;
  if (!table) {
    table = crc32.table = new Int32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      table[n] = c;
    }
  }
  let c = 0xffffffff;
  for (const b of buf) c = table[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

/** 纯色 PNG（8-bit truecolor，免第三方库；panorama/portrait 尺寸对齐移动端的溢出场景） */
function solidPng(width, height, [r, g, b]) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // truecolor
  const raw = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y++) {
    const row = y * (width * 3 + 1);
    raw[row] = 0; // filter: none
    for (let x = 0; x < width; x++) {
      raw[row + 1 + x * 3] = r;
      raw[row + 2 + x * 3] = g;
      raw[row + 3 + x * 3] = b;
    }
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', deflateSync(raw)),
    pngChunk('IEND', Buffer.alloc(0)),
  ]);
}

function seedLibrary(dir) {
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  // 宽高比差异大的组合：齐行布局在窄屏最容易把行推出视口
  const files = [
    ['panorama.png', 1200, 400, [79, 140, 255]],
    ['portrait.png', 400, 1200, [229, 83, 75]],
    ['square.png', 800, 800, [120, 200, 120]],
    ['wide.png', 1600, 500, [200, 160, 60]],
  ];
  for (const [name, w, h, rgb] of files) {
    fs.writeFileSync(path.join(dir, name), solidPng(w, h, rgb));
  }
}

function resolveServer() {
  if (process.env.HAWK_SERVER_EXE) {
    return { command: process.env.HAWK_SERVER_EXE, args: [] };
  }
  const dll = path.resolve(root, '..', 'hawk-server', 'bin', 'Debug', 'net10.0', 'hawk-server.dll');
  if (fs.existsSync(dll)) {
    return { command: 'dotnet', args: [dll] };
  }
  return null;
}

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

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitForHealth(port, deadlineMs) {
  const t0 = Date.now();
  for (;;) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/health`);
      if (res.ok) return;
    } catch {
      // 未监听，继续等
    }
    if (Date.now() - t0 > deadlineMs) throw new Error('hawk-server /health 等待超时');
    await wait(200);
  }
}

// ---------- 主流程 ----------
const checks = [];
const check = (ok, label, detail = '') => {
  checks.push({ ok, label, detail });
  console.log(`${ok ? '✓' : '✗'} ${label}${detail ? `（${detail}）` : ''}`);
};

let server = null;
let probe = null;
let failed = false;

try {
  // 1) 构建前端（测试的是最新代码 + 最新产物的组合）
  console.log('构建前端…');
  const build = spawnSync('npm', ['run', 'build'], { cwd: root, stdio: 'inherit', shell: process.platform === 'win32' });
  if (build.status !== 0) {
    throw new Error('前端构建失败');
  }

  // 2) 临时素材库 + server
  const libDir = path.join(tmpDir, 'library');
  seedLibrary(libDir);
  const serverCmd = resolveServer();
  if (!serverCmd) {
    throw new Error('未找到 hawk-server（先 dotnet build hawk-server，或设置 HAWK_SERVER_EXE）');
  }
  const token = crypto.randomBytes(32).toString('hex');
  const port = await probeFreePort();
  server = spawn(
    serverCmd.command,
    [...serverCmd.args, '--library', libDir, '--port', String(port), '--web-dist', path.join(root, 'web', 'dist')],
    { env: { ...process.env, HAWK_TOKEN: token }, stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true },
  );
  let serverErr = '';
  server.stderr.on('data', (c) => (serverErr += c));
  await waitForHealth(port, 30000);

  // 3) 手机浏览器探针
  fs.mkdirSync(shotsDir, { recursive: true });
  const electronBin = require('electron');
  const url = `http://127.0.0.1:${port}/?token=${token}`;
  probe = spawn(electronBin, [path.join(root, 'scripts', 'mobile-web-probe.cjs')], {
    env: { ...process.env, PROBE_URL: url, PROBE_DIR: shotsDir },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const ticks = [];
  let grid = null;
  let titlebar = null;
  let preview = null;
  let loadFailed = null;
  let timedOut = false;
  await new Promise((resolvePromise) => {
    let buf = '';
    probe.stdout.on('data', (chunk) => {
      buf += chunk;
      let idx;
      while ((idx = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, idx).trim();
        buf = buf.slice(idx + 1);
        if (!line.startsWith('{')) continue;
        const msg = JSON.parse(line);
        if (msg.type === 'tick') ticks.push(msg);
        else if (msg.type === 'grid') grid = msg;
        else if (msg.type === 'titlebar') titlebar = msg;
        else if (msg.type === 'preview') preview = msg;
        else if (msg.type === 'load-failed') loadFailed = msg;
        else if (msg.type === 'timeout') timedOut = true;
      }
    });
    probe.stderr.on('data', () => {});
    probe.on('exit', resolvePromise);
    setTimeout(resolvePromise, 90000);
  });

  // 4) 断言
  if (loadFailed) {
    check(false, '页面加载', JSON.stringify(loadFailed));
  } else {
    const sawStarting = ticks.some((t) => t.starting);
    const sawApp = ticks.some((t) => t.app);
    const sawError = ticks.some((t) => t.error);
    check(sawStarting, '启动屏渲染', sawStarting ? `t=${ticks.find((t) => t.starting).t}ms` : '未出现');
    check(!sawError, '无错误屏');
    // 核心回归检测：启动屏出现后必须进入主界面（浏览器轮询路径）
    check(sawApp, '轮询就绪进入主界面', sawApp ? `t=${ticks.find((t) => t.app).t}ms` : timedOut ? '45s 超时，卡在启动屏' : '未进入');
    check(ticks.every((t) => t.mobile), '移动端判定（body.mobile）');
    if (grid) {
      check(!grid.error && grid.cards >= 3, '网格卡片渲染', `cards=${grid.cards ?? '?'}`);
      check(!grid.error && grid.scrollWidth <= grid.innerWidth + 1, '无横向溢出', `scrollWidth=${grid.scrollWidth} / innerWidth=${grid.innerWidth}`);
    } else {
      check(false, '网格探针', '未执行');
    }
    if (titlebar) {
      check(
        !titlebar.error && titlebar.moreVisible && titlebar.searchBtnVisible && titlebar.searchBoxHidden &&
          titlebar.sortHidden && titlebar.filterHidden && titlebar.menuOpened && titlebar.hasFilterItem &&
          titlebar.hasSortItem && titlebar.searchOverlayOpened && titlebar.searchInputFocused && titlebar.searchOverlayClosed,
        '窄屏顶栏入口（排序筛选菜单/搜索退化）',
        titlebar.error ? titlebar.message : `menu=${titlebar.menuOpened} searchOverlay=${titlebar.searchOverlayOpened}`,
      );
    } else if (sawApp) {
      check(false, '窄屏顶栏入口（排序筛选菜单/搜索退化）', '探针未执行');
    }
    if (preview) {
      check(!preview.error && preview.overlay, '双击打开预览');
      check(!preview.error && preview.centerInViewport, '预览中央图在视口内', preview.error ? '' : `vw=${preview.vw} vh=${preview.vh}`);
    } else if (sawApp) {
      check(false, '预览探针', '未执行');
    }
  }
} catch (e) {
  failed = true;
  check(false, '测试执行', e instanceof Error ? e.message : String(e));
} finally {
  if (probe && !probe.killed) probe.kill();
  if (server && !server.killed) server.kill();
}

failed = failed || checks.some((c) => !c.ok);
if (failed) {
  console.log(`\n失败：产物保留在 ${tmpDir}（截图在 shots/），供排查`);
  process.exit(1);
}
fs.rmSync(tmpDir, { recursive: true, force: true });
console.log('\n移动端网页冒烟测试全部通过');

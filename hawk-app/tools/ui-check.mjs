// hawk-app UI 端到端自检：真实启动 vite + electron + hawk-daemon（临时素材库），
// 通过 Chrome DevTools Protocol 断言 DOM、模拟交互、验证 SSE 实时性，并截图。
// 用法：node tools/ui-check.mjs（需先 cargo build --release hawk-daemon）
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(import.meta.url);
const tmp = path.join(root, 'tools', '.tmp');
const lib = path.join(tmp, 'library');

// ---------- 工具 ----------

function crc32(buf) {
  let table = crc32.table;
  if (!table) {
    table = crc32.table = Array.from({ length: 256 }, (_, n) => {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      return c >>> 0;
    });
  }
  let c = 0xffffffff;
  for (const b of buf) c = table[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function png(w, h, [r, g, b]) {
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([len, body, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; ihdr[9] = 2; // 8-bit RGB
  const row = Buffer.concat([Buffer.from([0]), Buffer.alloc(w * 3).map((_, i) => [r, g, b][i % 3])]);
  const raw = Buffer.concat(Array.from({ length: h }, () => row));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

async function waitFor(fn, timeoutMs, interval = 300) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      const result = await fn();
      if (result) return result;
    } catch {
      // 页面重载期间执行上下文暂不存在，忽略后继续轮询
    }
    if (Date.now() > deadline) throw new Error('等待超时');
    await new Promise((r) => setTimeout(r, interval));
  }
}

let pass = 0;
let fail = 0;
function check(name, actual, expected) {
  if (actual === expected) {
    pass++;
    console.log(`ok   - ${name}`);
  } else {
    fail++;
    console.log(`FAIL - ${name}: 期望 [${expected}] 实际 [${actual}]`);
  }
}

// ---------- 准备素材库 ----------

fs.rmSync(tmp, { recursive: true, force: true });
fs.mkdirSync(path.join(lib, '海报'), { recursive: true });
fs.writeFileSync(path.join(lib, 'sunset.png'), png(4, 2, [255, 0, 0]));
fs.writeFileSync(path.join(lib, 'big.png'), png(1600, 1200, [128, 128, 128])); // 原图预览断言用（宽 > 1024 缩略图）
for (let i = 1; i <= 6; i++) {
  fs.writeFileSync(path.join(lib, `f${i}.png`), png(4, 2, [255, i * 30, 0])); // 填充多行网格
}
fs.writeFileSync(path.join(lib, '海报', 'cat.png'), png(2, 4, [0, 255, 0]));
fs.writeFileSync(path.join(lib, '海报', 'logo.png'), png(8, 8, [0, 0, 255]));

// 预设素材库配置，跳过目录选择框（按平台取 userData；跑完恢复原配置）
// Electron userData 的平台差异：win %APPDATA%\%name% / mac ~/Library/Application Support/%name% /
// linux $XDG_CONFIG_HOME（默认 ~/.config）/%name%；name 取 package.json 的 hawk-app
const configDir =
  process.platform === 'win32'
    ? path.join(process.env.APPDATA, 'hawk-app')
    : process.platform === 'darwin'
      ? path.join(os.homedir(), 'Library', 'Application Support', 'hawk-app')
      : path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config'), 'hawk-app');
fs.mkdirSync(configDir, { recursive: true });
const configFile = path.join(configDir, 'hawk-app.json');
const configBackup = fs.existsSync(configFile) ? fs.readFileSync(configFile, 'utf8') : null;
// 任何退出路径（含 spawn 失败导致的进程崩溃）都恢复原配置；原本无配置则删除本次写入，不残留
process.on('exit', () => {
  if (configBackup !== null) {
    fs.writeFileSync(configFile, configBackup);
  } else {
    fs.rmSync(configFile, { force: true });
  }
});
fs.writeFileSync(configFile, JSON.stringify({ libraryPath: lib }));

// ---------- 启动 vite + electron ----------

const vite = spawn('npm', ['run', 'dev:web'], { cwd: root, stdio: 'ignore', shell: process.platform === 'win32' });
let electron;

try {
  await waitFor(async () => (await fetch('http://localhost:5173/').catch(() => null))?.ok, 60_000);

  const electronBin = require('electron');
  electron = spawn(
    electronBin,
    [
      '.',
      '--remote-debugging-port=9222',
      // Ubuntu ≥24.04（GitHub runner）经 AppArmor 限制非特权 user namespace，npm 安装的 electron
      // 又没有 setuid 的 chrome-sandbox 助手，不关沙箱会启动即退（CDP 端点永远不出现）
      ...(process.platform === 'linux' ? ['--no-sandbox'] : []),
    ],
    {
      cwd: root,
      // 启动失败（沙箱/缺动态库）时让报错直接进日志，而不是只剩一句「等待超时」
      stdio: 'inherit',
      // 对账间隔缩短到 3s：监听静默丢事件时自检也能快速收敛
      env: { ...process.env, HAWK_RESCAN_INTERVAL: '3' },
    },
  );

  // ---------- 连接 CDP ----------

  const targets = await waitFor(async () => {
    const list = await fetch('http://127.0.0.1:9222/json').catch(() => null);
    if (!list?.ok) return null;
    const pages = await list.json();
    return pages.find((t) => t.type === 'page' && t.url.includes('localhost:5173'));
  }, 60_000);

  const ws = new WebSocket(targets.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });

  let msgId = 0;
  const pending = new Map();
  ws.onmessage = (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
    }
  };
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++msgId;
      pending.set(id, (msg) => (msg.error ? reject(new Error(msg.error.message)) : resolve(msg.result)));
      ws.send(JSON.stringify({ id, method, params }));
    });
  const evaljs = async (expression) => {
    const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    return result.result?.value;
  };
  const screenshot = async (name) => {
    const { data } = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(tmp, name), Buffer.from(data, 'base64'));
  };

  await send('Page.enable');
  await send('Runtime.enable');

  // 视图记忆存于 localStorage（Electron 用户目录跨运行保留），先清掉再重载，保证每轮自检从干净状态开始
  await evaljs(`localStorage.clear()`);
  await send('Page.reload');
  await waitFor(async () => evaljs(`document.readyState === 'complete' && !!document.querySelector('.app')`), 30_000);

  // ---------- 断言 ----------

  // 初始渲染：网格、侧栏、工具栏
  const cardCount = await waitFor(async () => {
    const n = await evaljs(`document.querySelectorAll('.card').length`);
    return n >= 10 ? n : null;
  }, 30_000);
  check('网格渲染 10 个素材卡片', cardCount, 10);
  check('侧栏显示文件夹', await evaljs(`document.querySelector('.sidebar .tree')?.textContent?.includes('海报') ?? false`), true);
  check('侧栏入口', await evaljs(`document.querySelector('.sidebar')?.textContent?.includes('回收站') ?? false`), true);
  check('位置标题为全部素材', await evaljs(`document.querySelector('.titlebar .title')?.textContent`), '全部素材');
  // 窗口控制 fixed 于窗口右上角；macOS 用系统原生红绿灯，不自绘
  check('窗口控制按钮（最小化/最大化/关闭）', await evaljs(`document.querySelectorAll('.win-controls .win-btn').length`), process.platform === 'darwin' ? 0 : 3);

  // ---- Eagle 式布局：左右栏通高到窗口上沿，标题栏只覆盖中栏 ----
  const geo = await evaljs(`(() => {
    const rect = (s) => document.querySelector(s).getBoundingClientRect();
    const sidebar = rect('.sidebar'); const titlebar = rect('.titlebar'); const inspector = rect('.inspector');
    return { sidebarTop: sidebar.top, inspectorTop: inspector.top,
      gapLeft: titlebar.left - sidebar.right, gapRight: inspector.left - titlebar.right };
  })()`);
  check('侧栏通高（顶到窗口上沿）', geo.sidebarTop, 0);
  check('检查器通高（顶到窗口上沿）', geo.inspectorTop, 0);
  check('标题栏左接侧栏右缘', Math.abs(geo.gapLeft) <= 1, true);
  check('标题栏右接检查器左缘', Math.abs(geo.gapRight) <= 1, true);

  // ---- 分区折叠：点分区标题收起/展开（v-show 保留树节点状态） ----
  await evaljs(`[...document.querySelectorAll('.sidebar .section')].find((s) => s.textContent.includes('文件夹'))?.click()`);
  check('文件夹分区收起', await evaljs(`getComputedStyle(document.querySelector('.sidebar .tree')).display`), 'none');
  await evaljs(`[...document.querySelectorAll('.sidebar .section')].find((s) => s.textContent.includes('文件夹'))?.click()`);
  check('文件夹分区展开', await evaljs(`getComputedStyle(document.querySelector('.sidebar .tree')).display`) !== 'none', true);
  check('后退按钮初始禁用', await evaljs(`document.querySelector('.titlebar .bar-btn[title="后退"]')?.disabled ?? false`), true);
  const badge = await waitFor(async () => {
    const value = await evaljs(`document.querySelector('.sidebar .entry .count')?.textContent`);
    return value === '10' ? value : null;
  }, 10_000).catch(async () => {
    // 诊断：后端 folder/list 的 count 与侧栏 HTML，定位是数据问题还是渲染/时序问题
    const count = await evaljs(`fetch(new URLSearchParams(location.hash.slice(1)).get('api') + '/api/v1/folder/list', { headers: { Authorization: 'Bearer ' + new URLSearchParams(location.hash.slice(1)).get('token') } }).then((r) => r.json()).then((e) => e.data.count).catch(() => 'ERR')`);
    const html = await evaljs(`document.querySelector('.sidebar .entry')?.outerHTML?.slice(-200)`);
    console.log(`  [诊断] folder/list count=${count}，首个 .entry 尾部 HTML=${html}`);
    return null;
  });
  check('全部素材计数徽章', badge, '10');

  // ---- 齐行网格：同一行内等高、每张卡宽高比与原图一致 ----
  const layout = await evaljs(`(() => {
    const thumbs = [...document.querySelectorAll('.card .thumb')];
    const byTop = new Map();
    for (const t of thumbs) {
      const r = t.getBoundingClientRect();
      const key = Math.round(r.top);
      if (!byTop.has(key)) byTop.set(key, []);
      byTop.get(key).push(r);
    }
    const rowEqual = [...byTop.values()].every((row) => row.every((r) => Math.abs(r.height - row[0].height) <= 1));
    const ratioOf = (name) => {
      const r = thumbs.find((t) => t.closest('.card').querySelector('.name')?.textContent?.startsWith(name + '.')).getBoundingClientRect();
      return r.width / r.height;
    };
    return { rowEqual, sunset: ratioOf('sunset'), cat: ratioOf('cat'), logo: ratioOf('logo') };
  })()`);
  check('齐行：行内等高', layout.rowEqual, true);
  check('齐行：宽高比跟随原图（4×2 ≈ 2）', Math.abs(layout.sunset - 2) < 0.1, true);
  check('齐行：宽高比跟随原图（2×4 ≈ 0.5）', Math.abs(layout.cat - 0.5) < 0.05, true);
  check('齐行：方形图宽高相等', Math.abs(layout.logo - 1) < 0.05, true);

  // 缩略图真实加载（自然宽度 > 0）；共享 runner 上首次索引/缩略图生成远超 2s，轮询等待而非固定 sleep
  const thumbsLoaded = await waitFor(async () => {
    const ok = await evaljs(`[...document.querySelectorAll('.card img')].every((i) => i.naturalWidth > 0)`);
    return ok ? true : null;
  }, 30_000).catch(async () => {
    // 诊断：complete=false 是仍在加载（生成慢）；complete=true 且 naturalWidth=0 是端点报错
    const state = await evaljs(`[...document.querySelectorAll('.card img')].filter((i) => i.naturalWidth === 0).map((i) => ({ id: i.src.match(/id=([0-9a-f]+)/)?.[1] ?? '', complete: i.complete }))`);
    console.log('  [诊断] 未就绪缩略图:', JSON.stringify(state));
    return false;
  });
  check('缩略图加载成功', thumbsLoaded, true);
  await screenshot('ui-grid.png');

  // ---- 预览浮层：空格展开原图、滚轮缩放、双击复位、预览内 ←→ 切换 ----
  await evaljs(`[...document.querySelectorAll('.card')].find((c) => c.querySelector('.name')?.textContent?.startsWith('big.'))?.click()`);
  await evaljs(`window.dispatchEvent(new KeyboardEvent('keydown', { key: ' ' }))`);
  // carousel 结构（scale=1）：track-row 三段为 前|当前|后，中间子元素恒为当前图
  const curImg = `.overlay .track-row > *:nth-child(2)`;
  const overlayReady = await waitFor(async () => evaljs(`!!document.querySelector('${curImg}')`), 5_000);
  check('空格展开预览浮层', overlayReady, true);
  check('预览使用原图端点', await evaljs(`document.querySelector('${curImg}').src.includes('/api/v1/item/file')`), true);
  const naturalWidth = await waitFor(async () => {
    const w = await evaljs(`document.querySelector('${curImg}').naturalWidth`);
    return w > 0 ? w : null;
  }, 10_000);
  check('预览加载原图（1600px，非 1024 缩略图）', naturalWidth, 1600);

  await evaljs(`document.querySelector('.overlay').dispatchEvent(new WheelEvent('wheel', { deltaY: -240, clientX: 720, clientY: 450, bubbles: true, cancelable: true }))`);
  const transform = await evaljs(`document.querySelector('.overlay .image').style.transform`);
  check('滚轮放大（scale > 1）', Number(transform.match(/scale\(([\d.]+)\)/)?.[1]) > 1, true);
  // dblclick 绑定在手势层（.gesture）；复位后平移层（v-if scale>1）卸载、carousel 轨道回归
  await evaljs(`document.querySelector('.overlay .gesture').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))`);
  check('双击复位缩放', await evaljs(`document.querySelector('.overlay .image') === null && !!document.querySelector('.overlay .track-row')`), true);

  const caption0 = await evaljs(`document.querySelector('.overlay .page-index')?.textContent ?? ''`);
  await evaljs(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight' }))`);
  const caption1 = await waitFor(async () => {
    const t = await evaljs(`document.querySelector('.overlay .page-index')?.textContent ?? ''`);
    return t !== '' && t !== caption0 ? t : null;
  }, 5_000).catch(() => '');
  check('预览中 → 切换下一张', caption1 !== '' && caption1 !== caption0, true);
  await screenshot('ui-preview.png');

  // 预览浮层右键菜单同样提供「编辑图片…」入口（网格与预览均可打开编辑窗口）
  await evaljs(`document.querySelector('.overlay').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 720, clientY: 450 }))`);
  check(
    '预览右键菜单含编辑图片',
    await waitFor(async () => evaljs(`[...document.querySelectorAll('.menu .item')].some((b) => b.textContent.includes('编辑图片'))`), 5_000),
    true,
  );
  await evaljs(`document.querySelector('.mask')?.click()`); // 收起菜单

  await evaljs(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }))`);
  check('Esc 关闭预览', await evaljs(`!document.querySelector('.overlay')`), true);

  // ---- 设置面板：打开 → 分区导航 → 局域网分区（子组件保活挂载）→ 关闭 ----
  await evaljs(`document.querySelector('.titlebar [title="设置"]')?.click()`);
  check('设置面板打开', await waitFor(async () => evaljs(`!!document.querySelector('.dialog .nav')`), 5_000), true);
  await evaljs(`[...document.querySelectorAll('.dialog .nav-item')].find((b) => b.textContent.includes('局域网'))?.click()`);
  check('局域网分区渲染（v-show 切换）', await waitFor(async () => {
    const el = await evaljs(`[...document.querySelectorAll('.pane')].find((p) => p.textContent.includes('启用局域网'))`);
    return el ? true : null;
  }, 5_000), true);
  await evaljs(`document.querySelector('.dialog-head .icon-btn')?.click()`);
  check('关闭设置面板', await evaljs(`!document.querySelector('.dialog')`), true);

  // ---- 图片编辑窗口：右键「编辑图片…」打开、旋转预览、脏退出三选确认 ----
  // 编辑窗口与预览浮层同为 body 级 .overlay,以 z-index 220 区分
  const editorOpen = async () => evaljs(`[...document.querySelectorAll('body > .overlay')].some((el) => getComputedStyle(el).zIndex === '220')`);
  await evaljs(`[...document.querySelectorAll('.card')].find((c) => c.querySelector('.name')?.textContent?.startsWith('sunset.'))?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 200, clientY: 200 }))`);
  check(
    '网格右键菜单含编辑图片',
    await waitFor(async () => evaljs(`[...document.querySelectorAll('.menu .item')].some((b) => b.textContent.includes('编辑图片'))`), 5_000),
    true,
  );
  await evaljs(`[...document.querySelectorAll('.menu .item')].find((b) => b.textContent.includes('编辑图片'))?.click()`);
  check('点击打开编辑窗口', await waitFor(async () => (await editorOpen()) ?? null, 5_000), true);
  check(
    '编辑窗口底部工具条（旋转/退出/保存）',
    await evaljs(`[...document.querySelectorAll('.bar button')].map((b) => b.textContent).join(',')`),
    '↺,↻,退出,保存',
  );
  await evaljs(`[...document.querySelectorAll('.bar button')].find((b) => b.textContent === '↻')?.click()`);
  await new Promise((r) => setTimeout(r, 200));
  check('旋转预览生效', await evaljs(`document.querySelector('body > .overlay .image')?.style.transform ?? ''`), 'rotate(90deg)');
  await evaljs(`[...document.querySelectorAll('.bar button')].find((b) => b.textContent === '退出')?.click()`);
  check('脏退出弹三选确认', await evaljs(`!!document.querySelector('.confirm-text')`), true);
  await evaljs(`[...document.querySelectorAll('.confirm-actions button')].find((b) => b.textContent === '不保存')?.click()`);
  await new Promise((r) => setTimeout(r, 300));
  check('放弃修改关闭编辑窗口', await editorOpen(), false);
  await screenshot('ui-edit.png');

  // ---- 方向键移动选中框 ----
  await evaljs(`document.querySelectorAll('.card')[0].click()`);
  const selId0 = await evaljs(`document.querySelector('.card.selected')?.dataset.itemId ?? ''`);
  await evaljs(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight' }))`);
  const selId1 = await evaljs(`document.querySelector('.card.selected')?.dataset.itemId ?? ''`);
  check('→ 选中移到下一项', selId1 !== '' && selId1 !== selId0, true);
  await evaljs(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft' }))`);
  check('← 选中移回上一项', await evaljs(`document.querySelector('.card.selected')?.dataset.itemId ?? ''`), selId0);
  await evaljs(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown' }))`);
  const selIdDown = await evaljs(`document.querySelector('.card.selected')?.dataset.itemId ?? ''`);
  check('↓ 选中移到下一行', selIdDown !== '' && selIdDown !== selId0, true);

  // 选中 → 检查器
  await evaljs(`document.querySelector('.card').click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.inspector .fields')`), 5_000);
  check('选中后检查器出现', await evaljs(`!!document.querySelector('.inspector .name-input')`), true);
  check('标题栏选中计数', await evaljs(`document.querySelector('.titlebar .selected-count')?.textContent?.includes('已选 1 项') ?? false`), true);
  await screenshot('ui-inspector.png');

  // 点星评分 → 卡片出现评分角标
  const firstName = await evaljs(`document.querySelector('.inspector .name-input').value`);
  await evaljs(`[...document.querySelectorAll('.inspector .rating .star')][4].click()`);
  const starMarked = await waitFor(async () => {
    const cards = await evaljs(
      `[...document.querySelectorAll('.card')].find((c) => c.querySelector('.name')?.textContent?.startsWith(${JSON.stringify(firstName)} + '.'))?.querySelector('.star')?.textContent ?? ''`,
    );
    return cards === '' ? null : cards;
  }, 5_000);
  check('评分同步到卡片角标', starMarked, '★5');

  // 文件夹视图切换
  await evaljs(`[...document.querySelectorAll('.sidebar .node')].find((n) => n.textContent.includes('海报'))?.click()`);
  const folderCount = await waitFor(async () => {
    const n = await evaljs(`document.querySelectorAll('.card').length`);
    return n === 2 ? n : null;
  }, 5_000);
  check('文件夹视图过滤', folderCount, 2);
  check('位置标题为文件夹名', await evaljs(`document.querySelector('.titlebar .crumb.current')?.textContent?.trim() ?? ''`), '海报');

  // ---- 标题栏前进/后退 ----
  await evaljs(`document.querySelector('.titlebar .bar-btn[title="后退"]').click()`);
  const backCount = await waitFor(async () => {
    const n = await evaljs(`document.querySelectorAll('.card').length`);
    return n === 10 ? n : null;
  }, 5_000);
  check('后退回全部素材', backCount, 10);
  await evaljs(`document.querySelector('.titlebar .bar-btn[title="前进"]').click()`);
  const fwdCount = await waitFor(async () => {
    const n = await evaljs(`document.querySelectorAll('.card').length`);
    return n === 2 ? n : null;
  }, 5_000);
  check('前进回文件夹视图', fwdCount, 2);

  // ---- 侧栏开关：可见时在侧栏顶条右端，隐藏时挪到顶栏左上角 ----
  check('侧栏可见时开关在侧栏顶条', await evaljs(`!!document.querySelector('.sidebar-head [title="侧栏与检查器"]')`), true);
  check('侧栏可见时顶栏无开关', await evaljs(`!!document.querySelector('.titlebar [title="侧栏与检查器"]')`), false);
  await evaljs(`document.querySelector('.sidebar-head [title="侧栏与检查器"]').click()`);
  check('侧栏隐藏', await evaljs(`getComputedStyle(document.querySelector('.sidebar')).display`), 'none');
  await screenshot('ui-no-panels.png');
  check('侧栏隐藏后开关在顶栏', await evaljs(`!!document.querySelector('.titlebar [title="侧栏与检查器"]')`), true);
  if (process.platform === 'darwin') {
    // macOS：顶栏通栏时左端避让窗口左上角的原生红绿灯
    check('顶栏左端避让红绿灯', await evaljs(`parseInt(getComputedStyle(document.querySelector('.titlebar')).paddingLeft)`), 78);
  }
  await evaljs(`document.querySelector('.titlebar [title="侧栏与检查器"]').click()`);
  check('侧栏恢复', await evaljs(`getComputedStyle(document.querySelector('.sidebar')).display`) !== 'none', true);
  if (process.platform === 'darwin') {
    check('侧栏恢复后顶栏无避让', await evaljs(`parseInt(getComputedStyle(document.querySelector('.titlebar')).paddingLeft)`), 10);
  }

  // SSE：另一进程写入文件 → 界面自动出现（先回全部素材）
  await evaljs(`[...document.querySelectorAll('.sidebar .entry')].find((n) => n.textContent.includes('全部素材'))?.click()`);
  await waitFor(async () => (await evaljs(`document.querySelectorAll('.card').length`)) === 10, 5_000);
  fs.writeFileSync(path.join(lib, 'sse-new.png'), png(3, 3, [255, 255, 0]));
  const sseCount = await waitFor(async () => {
    const n = await evaljs(`document.querySelectorAll('.card').length`);
    return n === 11 ? n : null;
  }, 15_000).catch(async () => {
    // 诊断：区分 SSE/刷新问题与监听丢事件——文件在盘上但后端没索引则探一下 reindex
    const cards = await evaljs(`document.querySelectorAll('.card').length`);
    const countOf = `fetch(new URLSearchParams(location.hash.slice(1)).get('api') + '/api/v1/item/count', { headers: { Authorization: 'Bearer ' + new URLSearchParams(location.hash.slice(1)).get('token') } }).then((r) => r.json()).then((e) => e.data).catch(() => -1)`;
    const before = await evaljs(countOf);
    await evaljs(`fetch(new URLSearchParams(location.hash.slice(1)).get('api') + '/api/v1/library/reindex', { method: 'POST', headers: { Authorization: 'Bearer ' + new URLSearchParams(location.hash.slice(1)).get('token') } })`);
    await new Promise((r) => setTimeout(r, 3000));
    const after = await evaljs(countOf);
    console.log(`  [诊断] 卡片数=${cards}，后端 item 数=${before}，reindex 后=${after}（reindex 能补上 → 监听静默丢事件）`);
    return null;
  });
  check('SSE 实时新增素材', sseCount, 11);

  // ---- 辅助：从卡片名取 item id（缩略图 URL 的 id 参数）与后端 detail ----
  const idByName = (name) =>
    `[...document.querySelectorAll('.card')].find((c) => c.querySelector('.name')?.textContent?.startsWith(${JSON.stringify(name)} + '.'))?.querySelector('img')?.src.match(/id=([0-9a-f]+)/)?.[1] ?? ''`;
  const fetchDetail = (id) =>
    `fetch('${targets.url ? '' : ''}' + new URLSearchParams(location.hash.slice(1)).get('api') + '/api/v1/item/detail?id=' + ${id}, { headers: { Authorization: 'Bearer ' + new URLSearchParams(location.hash.slice(1)).get('token') } }).then((r) => r.json()).then((e) => e.data)`;

  // ---- 右键菜单：包含 添加标签 / 移动到文件夹 ----
  await evaljs(`document.querySelector('.card').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 200, clientY: 200 }))`);
  await waitFor(async () => evaljs(`!!document.querySelector('.menu')`), 5_000);
  const menuText = await evaljs(`document.querySelector('.menu')?.textContent ?? ''`);
  check('右键菜单含「添加标签」', menuText.includes('添加标签'), true);
  check('右键菜单含「移动到文件夹」', menuText.includes('移动到文件夹'), true);

  // ---- 右键 → 添加标签 → 服务端与界面同步 ----
  const tagTarget = await evaljs(`document.querySelector('.inspector .name-input')?.value ?? ''`);
  await evaljs(`[...document.querySelectorAll('.menu .item')].find((b) => b.textContent.includes('添加标签'))?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.dialog input')`), 5_000);
  await evaljs(`(() => {
    const input = document.querySelector('.dialog input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '测试标签');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const tagOk = await waitFor(async () => {
    const id = await evaljs(idByName(tagTarget));
    if (!id) return null;
    const detail = await evaljs(await fetchDetail(`'${id}'`));
    return detail?.tags?.includes('测试标签') ? true : null;
  }, 5_000);
  check('右键添加标签生效（服务端）', tagOk, true);
  check('检查器标签 chip 显示', await evaljs(`[...document.querySelectorAll('.inspector .chip')].some((c) => c.textContent.includes('测试标签'))`), true);

  // ---- 右键 → 移动到文件夹（明确选根目录的 sunset，保证结果确定） ----
  await evaljs(`[...document.querySelectorAll('.card')].find((c) => c.querySelector('.name')?.textContent?.startsWith('sunset.'))?.click()`);
  await evaljs(`[...document.querySelectorAll('.card')].find((c) => c.querySelector('.name')?.textContent?.startsWith('sunset.'))?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 200, clientY: 200 }))`);
  await waitFor(async () => evaljs(`!!document.querySelector('.menu')`), 5_000);
  await evaljs(`[...document.querySelectorAll('.menu .item')].find((b) => b.textContent.includes('移动到文件夹'))?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.dialog select')`), 5_000);
  await evaljs(`(() => {
    const select = document.querySelector('.dialog select');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype, 'value').set;
    setter.call(select, '海报');
    select.dispatchEvent(new Event('change', { bubbles: true }));
  })()`);
  await evaljs(`[...document.querySelectorAll('.dialog .actions button')].find((b) => b.textContent === '确定')?.click()`);
  const moveTarget = await evaljs(`document.querySelector('.inspector .name-input')?.value ?? ''`);
  const moveOk = await waitFor(async () => {
    const id = await evaljs(idByName(moveTarget));
    if (!id) return null;
    const detail = await evaljs(await fetchDetail(`'${id}'`));
    return detail?.folders?.includes('海报') ? true : null;
  }, 5_000);
  check('移动到文件夹生效（服务端 folders）', moveOk, true);

  // ---- 新建根文件夹（侧栏「文件夹」区的 ＋ 按钮） ----
  await evaljs(`[...document.querySelectorAll('.sidebar .section')].find((s) => s.textContent.includes('文件夹'))?.querySelector('.add')?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.dialog input')`), 5_000);
  await evaljs(`(() => {
    const input = document.querySelector('.dialog input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '图标');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const folderCreated = await waitFor(async () => {
    const text = await evaljs(`document.querySelector('.sidebar .tree')?.textContent ?? ''`);
    return text.includes('图标') ? true : null;
  }, 5_000);
  check('侧栏新建根文件夹', folderCreated, true);

  // ---- 缩略图即时刷新：导入新图后无需手动刷新，缩略图自动就绪 ----
  await evaljs(`fetch(new URLSearchParams(location.hash.slice(1)).get('api') + '/api/v1/item/add', {
    method: 'POST',
    headers: { Authorization: 'Bearer ' + new URLSearchParams(location.hash.slice(1)).get('token'), 'Content-Type': 'application/json' },
    body: JSON.stringify({ img_base64: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', name: 'thumbcheck' }),
  })`);
  const thumbReady = await waitFor(async () => {
    const img = await evaljs(
      `[...document.querySelectorAll('.card')].find((c) => c.querySelector('.name')?.textContent?.startsWith('thumbcheck.'))?.querySelector('img')`,
    );
    if (!img) return null;
    return evaljs(`(() => { const img = [...document.querySelectorAll('.card')].find((c) => c.querySelector('.name')?.textContent?.startsWith('thumbcheck.'))?.querySelector('img'); return img && img.naturalWidth > 0; })()`);
  }, 15_000);
  check('导入后缩略图自动刷新', thumbReady, true);

  // ---- 视图记忆：进入文件夹视图后重载页面，应恢复该视图 ----
  await evaljs(`[...document.querySelectorAll('.sidebar .node')].find((n) => n.textContent.includes('海报'))?.click()`);
  await waitFor(async () => (await evaljs(`document.querySelectorAll('.card').length`)) === 3, 5_000);
  await send('Page.reload');
  // 等待针点用「侧栏出现 active 节点」（视图恢复完成的标志），而不是「cards > 0」——
  // 后者可能命中 reload 前的旧文档或恢复过程中的瞬态，误读卡片数
  const restoredCount = await waitFor(async () => {
    const ready = await evaljs(`document.readyState === 'complete' && !!document.querySelector('.sidebar .node.active')`);
    if (!ready) return null;
    const n = await evaljs(`document.querySelectorAll('.card').length`);
    return n > 0 ? n : null;
  }, 30_000);
  if (restoredCount !== 3) {
    // 诊断：区分「视图未恢复（回退全部）」与「视图恢复了但列表未过滤」
    const diag = await evaljs(`(() => ({
      storage: Object.entries(localStorage).map(([k, v]) => k + '=' + v),
      title: document.querySelector('.titlebar .title')?.textContent ?? '',
      crumb: document.querySelector('.titlebar .crumb.current')?.textContent?.trim() ?? '',
      activeNode: document.querySelector('.sidebar .node.active')?.textContent?.trim() ?? '',
      cards: [...document.querySelectorAll('.card .name')].map((n) => n.textContent),
    }))()`);
    console.log('  [诊断]', JSON.stringify(diag));
  }
  check('重载后恢复文件夹视图（卡片数）', restoredCount, 3);
  check(
    '重载后侧栏选中态',
    await evaljs(`document.querySelector('.sidebar .node.active')?.textContent?.includes('海报') ?? false`),
    true,
  );

  // ==================== 分类 / 标签维度 ====================
  // 回全部素材视图
  await evaljs(`[...document.querySelectorAll('.sidebar .entry')].find((n) => n.textContent.includes('全部素材'))?.click()`);
  await waitFor(async () => (await evaljs(`document.querySelectorAll('.card').length`)) >= 5, 5_000);

  // 新建空分类（侧栏＋；分类是扁平名字，Normalize 拒绝 '/'，见 category.md）
  await evaljs(`[...document.querySelectorAll('.sidebar .section')].find((s) => s.textContent.includes('分类'))?.querySelector('.add')?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.dialog input')`), 5_000);
  await evaljs(`(() => {
    const input = document.querySelector('.dialog input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '灵感');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const categoryCreated = await waitFor(async () => {
    const found = await evaljs(`[...document.querySelectorAll('.sidebar .tax-name')].some((n) => n.textContent.trim() === '灵感')`);
    return found ? true : null;
  }, 5_000);
  check('侧栏新建空分类', categoryCreated, true);

  // 右键第一张卡 → 添加到分类
  await evaljs(`document.querySelector('.card').click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.inspector .fields')`), 5_000);
  const catTarget = await evaljs(`document.querySelector('.inspector .name-input')?.value ?? ''`);
  await evaljs(`document.querySelector('.card').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 200, clientY: 200 }))`);
  await waitFor(async () => evaljs(`!!document.querySelector('.menu')`), 5_000);
  check('右键菜单含「添加到分类」', await evaljs(`document.querySelector('.menu')?.textContent?.includes('添加到分类') ?? false`), true);
  await evaljs(`[...document.querySelectorAll('.menu .item')].find((b) => b.textContent.includes('添加到分类'))?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.dialog input')`), 5_000);
  await evaljs(`(() => {
    const input = document.querySelector('.dialog input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '灵感');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const assignOk = await waitFor(async () => {
    const id = await evaljs(idByName(catTarget));
    if (!id) return null;
    const detail = await evaljs(await fetchDetail(`'${id}'`));
    return detail?.categories?.includes('灵感') ? true : null;
  }, 5_000);
  check('添加到分类生效（服务端）', assignOk, true);

  // 分类视图
  await evaljs(`[...document.querySelectorAll('.sidebar .tax-row')].find((r) => r.dataset.name === '灵感')?.click()`);
  const catViewCount = await waitFor(async () => {
    const n = await evaljs(`document.querySelectorAll('.card').length`);
    return n === 1 ? n : null;
  }, 5_000);
  check('分类视图过滤', catViewCount, 1);

  // 分类重命名（PromptDialog）：赋值跟随、视图跟随
  await evaljs(`[...document.querySelectorAll('.sidebar .tax-row')].find((r) => r.dataset.name === '灵感')?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 60, clientY: 60 }))`);
  await waitFor(async () => evaljs(`!!document.querySelector('.menu')`), 5_000);
  await evaljs(`[...document.querySelectorAll('.menu .item')].find((b) => b.textContent === '重命名')?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.dialog input')`), 5_000);
  await evaljs(`(() => {
    const input = document.querySelector('.dialog input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '灵感库');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const renameOk = await waitFor(async () => {
    const id = await evaljs(idByName(catTarget));
    if (!id) return null;
    const detail = await evaljs(await fetchDetail(`'${id}'`));
    return detail?.categories?.includes('灵感库') ? true : null;
  }, 5_000);
  check('分类重命名跟随（服务端）', renameOk, true);
  check('重命名后视图跟随', await evaljs(`document.querySelector('.sidebar .tax-row.active .tax-name')?.textContent?.trim() ?? ''`), '灵感库');

  // 删除分类（覆写 confirm）→ 赋值清除、视图回全部
  await evaljs(`window.confirm = () => true`);
  await evaljs(`[...document.querySelectorAll('.sidebar .tax-row')].find((r) => r.dataset.name === '灵感库')?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 60, clientY: 60 }))`);
  await waitFor(async () => evaljs(`!!document.querySelector('.menu')`), 5_000);
  await evaljs(`[...document.querySelectorAll('.menu .item')].find((b) => b.textContent.includes('删除分类'))?.click()`);
  const deleteOk = await waitFor(async () => {
    const id = await evaljs(idByName(catTarget));
    if (!id) return null;
    const detail = await evaljs(await fetchDetail(`'${id}'`));
    return detail && (detail.categories ?? []).length === 0 ? true : null;
  }, 5_000);
  check('删除分类清除赋值（服务端）', deleteOk, true);
  // 删除是异步 action（api → refreshTaxonomy → correctView），等视图收敛
  const backToAll = await waitFor(async () => {
    const t = await evaljs(`document.querySelector('.titlebar .title')?.textContent`);
    return t === '全部素材' ? t : null;
  }, 5_000);
  check('删除后视图回全部素材', backToAll, '全部素材');

  // 新建空标签 + 重命名跟随
  await evaljs(`[...document.querySelectorAll('.sidebar .section')].find((s) => s.textContent.includes('标签'))?.querySelector('.add')?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.dialog input')`), 5_000);
  await evaljs(`(() => {
    const input = document.querySelector('.dialog input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '预创建');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const tagCreated = await waitFor(async () => {
    const found = await evaljs(`[...document.querySelectorAll('.sidebar .tax-row')].some((r) => r.textContent.includes('预创建'))`);
    return found ? true : null;
  }, 5_000);
  check('侧栏新建空标签', tagCreated, true);

  // 重命名「测试标签」→「已测试」，item 跟随（用 item/list 验证，不依赖具体 item 身份）
  await evaljs(`[...document.querySelectorAll('.sidebar .tax-row')].find((r) => r.textContent.includes('测试标签'))?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 60, clientY: 60 }))`);
  await waitFor(async () => evaljs(`!!document.querySelector('.menu')`), 5_000);
  await evaljs(`[...document.querySelectorAll('.menu .item')].find((b) => b.textContent === '重命名')?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.dialog input')`), 5_000);
  await evaljs(`(() => {
    const input = document.querySelector('.dialog input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '已测试');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const tagRenameOk = await waitFor(async () => {
    const total = await evaljs(`fetch(new URLSearchParams(location.hash.slice(1)).get('api') + '/api/v1/item/list', {
      method: 'POST',
      headers: { Authorization: 'Bearer ' + new URLSearchParams(location.hash.slice(1)).get('token'), 'Content-Type': 'application/json' },
      body: JSON.stringify({ tags: ['已测试'] }),
    }).then((r) => r.json()).then((e) => e.data.total).catch(() => -1)`);
    return total === 1 ? true : null;
  }, 5_000);
  check('标签重命名跟随 item（服务端）', tagRenameOk, true);

  // ---- 多选批量：批量添加标签/分类 ----
  await evaljs(`document.querySelectorAll('.card')[0].dispatchEvent(new MouseEvent('click', { bubbles: true, ctrlKey: true }))`);
  await evaljs(`document.querySelectorAll('.card')[1].dispatchEvent(new MouseEvent('click', { bubbles: true, ctrlKey: true }))`);
  check('多选面板出现', await evaljs(`document.querySelector('.multi-title')?.textContent?.includes('已选 2') ?? false`), true);

  await evaljs(`(() => {
    const input = document.querySelector('.multi section input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '批量标签');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const batchTagOk = await waitFor(async () => {
    const total = await evaljs(`fetch(new URLSearchParams(location.hash.slice(1)).get('api') + '/api/v1/item/list', {
      method: 'POST',
      headers: { Authorization: 'Bearer ' + new URLSearchParams(location.hash.slice(1)).get('token'), 'Content-Type': 'application/json' },
      body: JSON.stringify({ tags: ['批量标签'] }),
    }).then((r) => r.json()).then((e) => e.data.total).catch(() => -1)`);
    return total === 2 ? true : null;
  }, 5_000);
  check('批量添加标签生效（服务端）', batchTagOk, true);

  await evaljs(`[...document.querySelectorAll('.multi .batch-btn')].find((b) => b.textContent.includes('添加到分类'))?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.dialog input')`), 5_000);
  await evaljs(`(() => {
    const input = document.querySelector('.dialog input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '批量');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const batchCatOk = await waitFor(async () => {
    const total = await evaljs(`fetch(new URLSearchParams(location.hash.slice(1)).get('api') + '/api/v1/item/list', {
      method: 'POST',
      headers: { Authorization: 'Bearer ' + new URLSearchParams(location.hash.slice(1)).get('token'), 'Content-Type': 'application/json' },
      body: JSON.stringify({ categories: ['批量'] }),
    }).then((r) => r.json()).then((e) => e.data.total).catch(() => -1)`);
    return total === 2 ? true : null;
  }, 5_000);
  check('批量添加分类生效（服务端）', batchCatOk, true);
  await evaljs(`document.body.click()`);

  // 回收站视图
  await evaljs(`[...document.querySelectorAll('.sidebar .entry')].find((n) => n.textContent.includes('回收站'))?.click()`);
  const trashEmpty = await waitFor(async () => {
    const text = await evaljs(`document.querySelector('.empty')?.textContent ?? ''`);
    return text === '' ? null : text;
  }, 5_000).catch(() => '');
  check('回收站空态', trashEmpty, '回收站为空');

  // 主进程素材库记忆：userData 配置应已写入当前库路径
  const cfg = JSON.parse(fs.readFileSync(configFile, 'utf8'));
  check('素材库路径已持久化', cfg.libraryPath === lib, true);
  await screenshot('ui-trash.png');

  console.log(`\n通过 ${pass} 项，失败 ${fail} 项`);
  console.log(`截图与产物在 ${tmp}`);
  process.exitCode = fail === 0 ? 0 : 1;
} catch (error) {
  console.error('自检异常:', error);
  process.exitCode = 1;
} finally {
  electron?.kill();
  vite.kill();
}

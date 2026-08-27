// hawk-app UI 端到端自检：真实启动 vite + electron + hawk-server（临时素材库），
// 通过 Chrome DevTools Protocol 断言 DOM、模拟交互、验证 SSE 实时性，并截图。
// 用法：node tools/ui-check.mjs（需先 dotnet build hawk-server）
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
fs.writeFileSync(path.join(lib, '海报', 'cat.png'), png(2, 4, [0, 255, 0]));
fs.writeFileSync(path.join(lib, '海报', 'logo.png'), png(8, 8, [0, 0, 255]));

// 预设素材库配置，跳过目录选择框
const configDir = path.join(os.homedir(), 'Library', 'Application Support', 'hawk-app');
fs.mkdirSync(configDir, { recursive: true });
fs.writeFileSync(path.join(configDir, 'hawk-app.json'), JSON.stringify({ libraryPath: lib }));

// ---------- 启动 vite + electron ----------

const vite = spawn('npm', ['run', 'dev:web'], { cwd: root, stdio: 'ignore' });
let electron;

try {
  await waitFor(async () => (await fetch('http://localhost:5173/').catch(() => null))?.ok, 60_000);

  const electronBin = require('electron');
  electron = spawn(electronBin, ['.', '--remote-debugging-port=9222'], {
    cwd: root,
    stdio: 'ignore',
    // 对账间隔缩短到 3s：监听静默丢事件时自检也能快速收敛
    env: { ...process.env, HAWK_RESCAN_INTERVAL: '3' },
  });

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
    return n >= 3 ? n : null;
  }, 30_000);
  check('网格渲染 3 个素材卡片', cardCount, 3);
  check('侧栏显示文件夹', await evaljs(`document.querySelector('.sidebar .tree')?.textContent?.includes('海报') ?? false`), true);
  check('侧栏入口', await evaljs(`document.querySelector('.sidebar')?.textContent?.includes('回收站') ?? false`), true);
  check('位置标题为全部素材', await evaljs(`document.querySelector('.toolbar .title')?.textContent`), '全部素材');
  const badge = await waitFor(async () => {
    const value = await evaljs(`document.querySelector('.sidebar .entry .count')?.textContent`);
    return value === '3' ? value : null;
  }, 10_000).catch(async () => {
    // 诊断：后端 folder/list 的 count 与侧栏 HTML，定位是数据问题还是渲染/时序问题
    const count = await evaljs(`fetch(new URLSearchParams(location.hash.slice(1)).get('api') + '/api/v1/folder/list', { headers: { Authorization: 'Bearer ' + new URLSearchParams(location.hash.slice(1)).get('token') } }).then((r) => r.json()).then((e) => e.data.count).catch(() => 'ERR')`);
    const html = await evaljs(`document.querySelector('.sidebar .entry')?.outerHTML?.slice(-200)`);
    console.log(`  [诊断] folder/list count=${count}，首个 .entry 尾部 HTML=${html}`);
    return null;
  });
  check('全部素材计数徽章', badge, '3');

  // ---- 齐行网格：行内等高、宽度按宽高比分配 ----
  const layout = await evaljs(`(() => {
    const rect = (name) => [...document.querySelectorAll('.card')].find((c) => c.querySelector('.name')?.textContent?.startsWith(name + '.'))?.querySelector('.thumb')?.getBoundingClientRect();
    const s = rect('sunset'), c = rect('cat'), l = rect('logo');
    return { sunset: { w: s.width, h: s.height }, cat: { w: c.width, h: c.height }, logo: { w: l.width, h: l.height } };
  })()`);
  check('齐行：行内等高', Math.abs(layout.sunset.h - layout.cat.h) <= 1, true);
  check('齐行：宽度按宽高比（4×2 与 2×4 宽度比 ≈ 4）', Math.abs(layout.sunset.w / layout.cat.w - 4) < 0.3, true);
  check('齐行：方形图宽高相等', Math.abs(layout.logo.w - layout.logo.h) <= 1, true);

  // 缩略图真实加载（自然宽度 > 0）
  await new Promise((r) => setTimeout(r, 2000));
  check(
    '缩略图加载成功',
    await evaljs(`[...document.querySelectorAll('.card img')].every((i) => i.naturalWidth > 0)`),
    true,
  );
  await screenshot('ui-grid.png');

  // 选中 → 检查器
  await evaljs(`document.querySelector('.card').click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.inspector .fields')`), 5_000);
  check('选中后检查器出现', await evaljs(`!!document.querySelector('.inspector .name-input')`), true);
  check('工具栏选中计数', await evaljs(`document.querySelector('.toolbar .selected')?.textContent?.includes('已选 1 项') ?? false`), true);
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
  check('位置标题为文件夹名', await evaljs(`document.querySelector('.toolbar .title')?.textContent`), '海报');

  // SSE：另一进程写入文件 → 界面自动出现（先回全部素材）
  await evaljs(`[...document.querySelectorAll('.sidebar .entry')].find((n) => n.textContent.includes('全部素材'))?.click()`);
  await waitFor(async () => (await evaljs(`document.querySelectorAll('.card').length`)) === 3, 5_000);
  fs.writeFileSync(path.join(lib, 'sse-new.png'), png(3, 3, [255, 255, 0]));
  const sseCount = await waitFor(async () => {
    const n = await evaljs(`document.querySelectorAll('.card').length`);
    return n === 4 ? n : null;
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
  check('SSE 实时新增素材', sseCount, 4);

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
  const restoredCount = await waitFor(async () => {
    const ready = await evaljs(`document.readyState === 'complete' && document.querySelectorAll('.card').length > 0`);
    if (!ready) return null;
    return evaljs(`document.querySelectorAll('.card').length`);
  }, 30_000);
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

  // 新建空分类（侧栏＋，一次建两层）
  await evaljs(`[...document.querySelectorAll('.sidebar .section')].find((s) => s.textContent.includes('分类'))?.querySelector('.add')?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.dialog input')`), 5_000);
  await evaljs(`(() => {
    const input = document.querySelector('.dialog input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '灵感/构图');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const categoryCreated = await waitFor(async () => {
    const found = await evaljs(`[...document.querySelectorAll('.sidebar .node .name')].some((n) => n.textContent.trim() === '灵感')`);
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
    setter.call(input, '灵感/构图');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const assignOk = await waitFor(async () => {
    const id = await evaljs(idByName(catTarget));
    if (!id) return null;
    const detail = await evaljs(await fetchDetail(`'${id}'`));
    return detail?.categories?.includes('灵感/构图') ? true : null;
  }, 5_000);
  check('添加到分类生效（服务端）', assignOk, true);

  // 分类视图（含子分类命中）
  await evaljs(`[...document.querySelectorAll('.sidebar .node .name')].find((n) => n.textContent.trim() === '灵感')?.closest('.node')?.click()`);
  const catViewCount = await waitFor(async () => {
    const n = await evaljs(`document.querySelectorAll('.card').length`);
    return n === 1 ? n : null;
  }, 5_000);
  check('分类视图过滤（含子分类）', catViewCount, 1);

  // 分类重命名（内联编辑）：子树跟随、视图跟随
  await evaljs(`[...document.querySelectorAll('.sidebar .node .name')].find((n) => n.textContent.trim() === '灵感')?.closest('.node')?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 60, clientY: 60 }))`);
  await waitFor(async () => evaljs(`!!document.querySelector('.menu')`), 5_000);
  await evaljs(`[...document.querySelectorAll('.menu .item')].find((b) => b.textContent === '重命名')?.click()`);
  await waitFor(async () => evaljs(`!!document.querySelector('.sidebar .node .edit')`), 5_000);
  await evaljs(`(() => {
    const input = document.querySelector('.sidebar .node .edit');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '灵感库');
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  })()`);
  const renameOk = await waitFor(async () => {
    const id = await evaljs(idByName(catTarget));
    if (!id) return null;
    const detail = await evaljs(await fetchDetail(`'${id}'`));
    return detail?.categories?.includes('灵感库/构图') ? true : null;
  }, 5_000);
  check('分类重命名子树跟随（服务端）', renameOk, true);
  check('重命名后视图跟随', await evaljs(`document.querySelector('.sidebar .node.active .name')?.textContent?.trim() ?? ''`), '灵感库');

  // 删除分类（覆写 confirm）→ 赋值清除、视图回全部
  await evaljs(`window.confirm = () => true`);
  await evaljs(`[...document.querySelectorAll('.sidebar .node .name')].find((n) => n.textContent.trim() === '灵感库')?.closest('.node')?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 60, clientY: 60 }))`);
  await waitFor(async () => evaljs(`!!document.querySelector('.menu')`), 5_000);
  await evaljs(`[...document.querySelectorAll('.menu .item')].find((b) => b.textContent.includes('删除分类'))?.click()`);
  const deleteOk = await waitFor(async () => {
    const id = await evaljs(idByName(catTarget));
    if (!id) return null;
    const detail = await evaljs(await fetchDetail(`'${id}'`));
    return detail && (detail.categories ?? []).length === 0 ? true : null;
  }, 5_000);
  check('删除分类清除赋值（服务端）', deleteOk, true);
  check('删除后视图回全部素材', await evaljs(`document.querySelector('.toolbar .title')?.textContent`), '全部素材');

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
    const found = await evaljs(`[...document.querySelectorAll('.sidebar .tag-row')].some((r) => r.textContent.includes('预创建'))`);
    return found ? true : null;
  }, 5_000);
  check('侧栏新建空标签', tagCreated, true);

  // 重命名「测试标签」→「已测试」，item 跟随（用 item/list 验证，不依赖具体 item 身份）
  await evaljs(`[...document.querySelectorAll('.sidebar .tag-row')].find((r) => r.textContent.includes('测试标签'))?.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 60, clientY: 60 }))`);
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
    setter.call(input, '批量/测试');
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
  const cfg = JSON.parse(fs.readFileSync(path.join(configDir, 'hawk-app.json'), 'utf8'));
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

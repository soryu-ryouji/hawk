// 拖拽功能端到端复现：真实浏览器（系统 Edge）+ 真实鼠标事件。
// 场景：多选两张素材 → 拖到侧栏文件夹 → 断言移动生效；过程中断言行高亮出现（证明 dragover MIME 检查通过）。
// 用法：node tools/dnd-test.mjs（需先 dotnet build + npm run build；临时库与进程自动清理）
import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import zlib from 'node:zlib';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const lib = mkdtempSync(path.join(tmpdir(), 'hawk-dnd-lib-'));
const port = 27481;
const token = 'dnd-test-token';
const base = `http://127.0.0.1:${port}`;

// --- 造库：3 张不同内容的 PNG，全部放根目录 ---
const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = (buf) => {
  let c = 0xffffffff;
  for (const b of buf) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
const chunk2 = (t, d) => {
  const td = Buffer.concat([t, d]);
  const len = Buffer.alloc(4);
  len.writeUInt32BE(d.length);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
};

function png(rgb) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(4, 0);
  ihdr.writeUInt32BE(4, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // truecolor
  const row = Buffer.concat([Buffer.from([0]), Buffer.from(rgb), Buffer.from(rgb), Buffer.from(rgb), Buffer.from(rgb)]);
  const raw = Buffer.concat([row, row, row, row]);
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk2(Buffer.from('IHDR'), ihdr),
    chunk2(Buffer.from('IDAT'), zlib.deflateSync(raw)),
    chunk2(Buffer.from('IEND'), Buffer.alloc(0)),
  ]);
}
mkdirSync(path.join(lib, 'sub'), { recursive: true });
writeFileSync(path.join(lib, 'red.png'), png([255, 0, 0]));
writeFileSync(path.join(lib, 'green.png'), png([0, 255, 0]));
writeFileSync(path.join(lib, 'blue.png'), png([0, 0, 255]));

const server = spawn('dotnet', [path.join(root, '..', 'hawk-server', 'bin', 'Debug', 'net10.0', 'hawk-server.dll'), '--library', lib, '--port', String(port)], {
  env: { ...process.env, HAWK_TOKEN: token },
  stdio: 'ignore',
});

// 静态伺服 web/dist（file:// 下 ES module 被 CORS 拦截，必须走 http）
const webPort = 27482;
const webServer = http.createServer((req, res) => {
  const file = path.join(root, 'web', 'dist', decodeURIComponent((req.url ?? '/').split('?')[0].split('#')[0]));
  const target = existsSync(file) && readFileSync(file) !== undefined ? (file.endsWith(path.sep) || !file.split(path.sep).pop()?.includes('.') ? path.join(file, 'index.html') : file) : null;
  if (!target || !existsSync(target)) {
    res.writeHead(404);
    res.end();
    return;
  }
  const ext = path.extname(target);
  const mime = ext === '.html' ? 'text/html' : ext === '.js' ? 'text/javascript' : ext === '.css' ? 'text/css' : 'application/octet-stream';
  res.writeHead(200, { 'content-type': mime });
  res.end(readFileSync(target));
});
await new Promise((r) => webServer.listen(webPort, '127.0.0.1', r));

const results = [];
const check = (name, ok) => {
  results.push([name, ok]);
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}`);
};

let browser;
let page;
const consoleErrors = [];
try {
  // 等就绪
  const deadline = Date.now() + 30000;
  for (;;) {
    try {
      const r = await fetch(`${base}/health`);
      if (r.ok) break;
    } catch { /* not up */ }
    if (Date.now() > deadline) throw new Error('server 启动超时');
    await new Promise((r) => setTimeout(r, 300));
  }

  browser = await chromium.launch({ channel: 'chrome', headless: true });
  page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  page.on('console', (m) => m.type() === 'error' && consoleErrors.push(m.text()));
  page.on('pageerror', (e) => consoleErrors.push(String(e)));
  // 拖拽事件探针：记录捕获阶段全部 drag 事件与 dataTransfer 状态 + 侧栏 DOM 变动
  await page.addInitScript(() => {
    window.__dragLog = [];
    window.__mutations = [];
    const startObserver = () => {
      new MutationObserver((records) => {
        for (const r of records) {
          if (r.type === 'childList' && (r.addedNodes.length || r.removedNodes.length)) {
            window.__mutations.push({
              t: Math.round(performance.now()),
              target: String(r.target.className ?? '').slice(0, 40),
              added: r.addedNodes.length,
              removed: r.removedNodes.length,
            });
          }
        }
      }).observe(document.body, { childList: true, subtree: true });
    };
    if (document.body) startObserver();
    else document.addEventListener('DOMContentLoaded', startObserver, { once: true });
    // dragover/drop 用冒泡阶段探测：document 冒泡晚于行内 Vue 处理器，能真实反映 defaultPrevented
    for (const t of ['dragstart', 'dragend', 'dragenter', 'dragleave']) {
      document.addEventListener(
        t,
        (e) => {
          const dt = e.dataTransfer;
          window.__dragLog.push({
            t: Math.round(performance.now()),
            type: t,
            target: String(e.target.className ?? '').slice(0, 50),
            types: dt ? [...dt.types] : null,
            defaultPrevented: e.defaultPrevented,
          });
        },
        true,
      );
    }
    for (const t of ['dragover', 'drop']) {
      document.addEventListener(t, (e) => {
        const dt = e.dataTransfer;
        window.__dragLog.push({
          t: Math.round(performance.now()),
          type: t,
          target: String(e.target.className ?? '').slice(0, 50),
          types: dt ? [...dt.types] : null,
          data: t === 'drop' && dt ? dt.getData('application/x-hawk-items') : undefined,
          defaultPrevented: e.defaultPrevented,
          at: String(document.elementFromPoint(e.clientX, e.clientY)?.className ?? '?').slice(0, 50),
        });
      });
    }
  });
  page.on('response', (r) => {
    if (r.url().includes('/api/v1/item/update')) {
      console.log(`> item/update ${r.status()} ${r.request().postData()}`);
    }
  });

  await page.goto(`http://127.0.0.1:${webPort}/index.html#api=${encodeURIComponent(base)}&token=${token}`);
  await page.waitForSelector('.card', { timeout: 15000 });
  await page.waitForTimeout(800);

  const cards = page.locator('.card');
  const count = await cards.count();
  check(`网格渲染 ${count} 张卡片`, count === 3);

  // 多选：点第 1 张，Ctrl+点第 2 张
  await cards.nth(0).click();
  await cards.nth(1).click({ modifiers: ['Control'] });
  await page.waitForTimeout(200);
  const selectedCount = await page.locator('.card.selected').count();
  check(`Ctrl 多选后选中 ${selectedCount} 张`, selectedCount === 2);

  // 拖第 1 张（已选中）到侧栏文件夹树 sub 行
  const sidebarFolder = page.locator('.sidebar .node', { hasText: 'sub' }).first();
  check('侧栏存在 sub 文件夹行', (await sidebarFolder.count()) === 1);

  const srcBox = await cards.nth(0).boundingBox();
  const dstBox = await sidebarFolder.boundingBox();
  check('卡片与目标都可定位', !!srcBox && !!dstBox);

  // 方式一：Playwright 专用 dragTo（CDP 级 HTML5 DnD，与真实浏览器行为一致）
  await cards.nth(0).dragTo(sidebarFolder, { force: true });
  await page.waitForTimeout(1500);
  const listAfterDragTo = await (
    await fetch(`${base}/api/v1/item/list`, {
      method: 'POST',
      headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
      body: JSON.stringify({ folders: ['sub'] }),
    })
  ).json();
  check(`dragTo 方式：sub 内 item 数 = ${listAfterDragTo.data.total}（期望 2）`, listAfterDragTo.data.total === 2);

  // 方式二：原始 mouse.down/move/up（对照：此方式在对照页可触发 drop）
  await page.evaluate(() => { window.__dragLog = []; window.__mutations = []; });
  await page.mouse.move(srcBox.x + srcBox.width / 2, srcBox.y + srcBox.height / 2);
  await page.mouse.down();
  await page.evaluate(() => {
    document.addEventListener('dragstart', (e) => { window.__srcNode = e.target; }, { once: true, capture: true });
  });
  for (let i = 1; i <= 10; i++) {
    await page.mouse.move(
      srcBox.x + ((dstBox.x + dstBox.width / 2 - srcBox.x) * i) / 10,
      srcBox.y + ((dstBox.y + dstBox.height / 2 - srcBox.y) * i) / 10,
    );
    await page.waitForTimeout(30);
  }
  await page.waitForTimeout(300);

  // drop 前：指针命中元素的完整信息 + 侧栏结构快照
  const probe = await page.evaluate(([x, y]) => {
    const el = document.elementFromPoint(x, y);
    const path = [];
    let cur = el;
    while (cur && path.length < 6) {
      path.push(cur.tagName + '.' + String(cur.className).split(' ').join('.'));
      cur = cur.parentElement;
    }
    const nodes = [...document.querySelectorAll('.sidebar .node')].map((n) => ({
      text: n.textContent?.slice(0, 20),
      rect: n.getBoundingClientRect().toJSON(),
    }));
    return { path, nodes };
  }, [dstBox.x + dstBox.width / 2, dstBox.y + dstBox.height / 2]);
  console.log('指针命中路径:', probe.path.join(' < '));
  console.log('侧栏文件夹行:', JSON.stringify(probe.nodes));

  // 拖拽悬停中：行应出现高亮（证明 dragover 通过 MIME 检查）
  const highlighted = await sidebarFolder.evaluate((el) => el.classList.contains('drop-target'));
  check('悬停时文件夹行高亮（dragover MIME 检查通过）', highlighted);

  await page.mouse.up();
  await page.waitForTimeout(200);
  const dragLog = await page.evaluate(() => window.__dragLog);
  const dragStart = dragLog.find((e) => e.type === 'dragstart')?.t ?? 0;
  const dragEnd = dragLog.find((e) => e.type === 'dragend')?.t ?? Infinity;
  console.log('--- drag 事件流 ---');
  for (const ev of dragLog.filter((e) => e.type !== 'dragover')) {
    console.log(JSON.stringify(ev));
  }
  const overs = dragLog.filter((e) => e.type === 'dragover');
  console.log(`dragover ${overs.length} 次, 最后:`, JSON.stringify(overs.at(-1)));
  const mutations = await page.evaluate(() => window.__mutations);
  const inDrag = mutations;
  console.log(`拖拽窗口内 DOM 变动 ${inDrag.length} 处:`);
  for (const m of inDrag.slice(0, 12)) console.log(JSON.stringify(m));
  const srcHealth = await page.evaluate(() => ({
    connected: window.__srcNode?.isConnected ?? 'no-dragstart',
    sameNode: window.__srcNode ? window.__srcNode === document.querySelector('.card.selected') : 'n/a',
  }));
  console.log('拖拽源节点状态:', JSON.stringify(srcHealth));
  const toast = await page.locator('.toast').textContent().catch(() => null);
  console.log('方式二 drop 后 toast:', toast ?? '(无)');
  await page.waitForTimeout(1500);

  // 断言移动结果：两张图进入 sub（通过 API 查文件夹过滤）
  const list = await (
    await fetch(`${base}/api/v1/item/list`, {
      method: 'POST',
      headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
      body: JSON.stringify({ folders: ['sub'] }),
    })
  ).json();
  check(`drop 后 sub 文件夹内 item 数 = ${list.data.total}（期望 2）`, list.data.total === 2);

  check('页面无致命错误（资源 404 如 favicon/索引中缩略图属预期）', consoleErrors.every((e) => e.includes('404')));
  if (consoleErrors.length) console.log('console errors:', consoleErrors.slice(0, 5));
} catch (e) {
  console.log('ERROR:', e.message);
  if (consoleErrors.length) console.log('console errors:', consoleErrors.slice(0, 8));
  await page?.screenshot({ path: path.join(root, 'tools', '.tmp-dnd-fail.png') }).catch(() => {});
  console.log('截图: hawk-app/tools/.tmp-dnd-fail.png');
} finally {
  await browser?.close();
  server.kill();
  webServer.close();
  rmSync(lib, { recursive: true, force: true });
  const failed = results.filter(([, ok]) => !ok).length;
  console.log(`\n${results.length - failed}/${results.length} 通过`);
  process.exit(failed ? 1 : 0);
}

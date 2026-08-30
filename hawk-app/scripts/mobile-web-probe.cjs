// 移动端网页冒烟探针（由 test-mobile-web.mjs 以子进程方式拉起）。
// 模拟手机浏览器环境：无 preload（window.hawkShell 不存在，走浏览器轮询分支）、390×844 视口。
// 向 stdout 输出 JSONL 探针（orchestrator 解析断言），关键节点截图落盘 PROBE_DIR。
const { app, BrowserWindow } = require('electron');
const fs = require('node:fs');

// 独立应用名：避免与开发中的 hawk（默认单实例锁/Electron 默认名）互相干扰
app.setName('hawk-mobile-probe');

const URL = process.env.PROBE_URL;
const DIR = process.env.PROBE_DIR;
const emit = (obj) => console.log(JSON.stringify(obj));

const TICK_SRC = `(() => ({
  starting: !!document.querySelector('.starting'),
  app: !!document.querySelector('.app'),
  error: !!document.querySelector('.error-text'),
  status: document.querySelector('.status')?.textContent ?? '',
  mobile: document.body.classList.contains('mobile'),
}))()`;

const GRID_SRC = `(() => ({
  cards: document.querySelectorAll('.card').length,
  placeholders: document.querySelectorAll('.cell-placeholder').length,
  innerWidth: window.innerWidth,
  scrollWidth: document.documentElement.scrollWidth,
}))()`;

// carousel 模式下中央帧（index 1）必须完整落在视口内（前/后邻图在屏外属设计行为，不看）
const PREVIEW_SRC = `(() => {
  const overlay = document.querySelector('.overlay');
  const imgs = [...document.querySelectorAll('.overlay .track-img')];
  const vw = window.innerWidth, vh = window.innerHeight;
  const center = imgs[1] ?? imgs[0];
  const r = center?.getBoundingClientRect();
  return {
    overlay: !!overlay,
    imgCount: imgs.length,
    centerInViewport: !!r && r.left >= -1 && r.right <= vw + 1 && r.top >= -1 && r.bottom <= vh + 1,
    vw,
    vh,
  };
})()`;

app.whenReady().then(async () => {
  const win = new BrowserWindow({
    width: 390,
    height: 844,
    show: false,
    // sandbox 渲染进程无 Node 能力，最接近真实手机浏览器
    webPreferences: { sandbox: true },
  });
  const shot = async (name) => {
    try {
      const img = await win.webContents.capturePage();
      fs.writeFileSync(`${DIR}/${name}.png`, img.toPNG());
    } catch {
      // 截图失败不阻塞探针
    }
  };

  win.webContents.on('did-fail-load', (_e, code, desc) => {
    emit({ type: 'load-failed', code, desc });
    app.exit(3);
  });

  try {
    await win.loadURL(URL);
  } catch (e) {
    emit({ type: 'load-failed', message: String(e) });
    app.exit(3);
    return;
  }

  const t0 = Date.now();
  let shotStartup = false;
  let shotGrid = false;
  const timer = setInterval(async () => {
    const p = await win.webContents.executeJavaScript(TICK_SRC).catch(() => null);
    if (!p) {
      return;
    }
    emit({ type: 'tick', t: Date.now() - t0, ...p });
    if (p.starting && !shotStartup) {
      shotStartup = true;
      await shot('startup');
    }
    if (!p.app || shotGrid) {
      if (Date.now() - t0 > 45000) {
        clearInterval(timer);
        emit({ type: 'timeout' });
        app.exit(2);
      }
      return;
    }
    // 主界面就绪：骨架→行布局→卡片渲染有异步窗口，.app 出现后立即探针会撞上 cards=0，
    // 先轮询等网格首帧（卡片或占位块）出现；点按开预览仅 narrow+touch（pointer: coarse），
    // 探针窗口为精细指针无法模拟，走窄窗鼠标路径（双击打开）验证预览链路
    shotGrid = true;
    clearInterval(timer);
    const gridDeadline = Date.now();
    let grid = null;
    while (Date.now() - gridDeadline < 15000) {
      grid = await win.webContents.executeJavaScript(GRID_SRC).catch(() => null);
      if (grid && !grid.error && grid.cards + grid.placeholders > 0) {
        break;
      }
      await new Promise((r) => setTimeout(r, 200));
    }
    emit({ type: 'grid', t: Date.now() - t0, ...(grid ?? { error: true }) });
    await shot('grid');
    const cardDeadline = Date.now();
    while (Date.now() - cardDeadline < 15000) {
      const hasCard = await win.webContents
        .executeJavaScript(`!!document.querySelector('.card')`)
        .catch(() => false);
      if (hasCard) {
        break;
      }
      await new Promise((r) => setTimeout(r, 200));
    }
    await win.webContents.executeJavaScript(
      `document.querySelector('.card')?.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))`,
    );
    setTimeout(async () => {
      const v = await win.webContents.executeJavaScript(PREVIEW_SRC).catch(() => null);
      emit({ type: 'preview', t: Date.now() - t0, ...(v ?? { error: true }) });
      await shot('preview');
      app.exit(0);
    }, 1200);
  }, 400);
});

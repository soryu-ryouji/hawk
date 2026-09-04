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

// 窄屏顶栏：排序/筛选按钮与宽屏一致直出（图标统一）；搜索框退化为按钮 + 浮层
const TITLEBAR_SRC = `(async () => {
  const visible = (el) => !!el && getComputedStyle(el).display !== 'none' && el.getBoundingClientRect().width > 0;
  const raf2 = () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  try {
    const sortBtn = document.querySelector('.titlebar .sort-btn');
    const filterBtn = document.querySelector('.titlebar .filter-btn');
    const searchBtn = document.querySelector('.titlebar .mobile-search-btn');
    const base = {
      sortVisible: visible(sortBtn),
      filterVisible: visible(filterBtn),
      searchBtnVisible: visible(searchBtn),
      searchBoxHidden: !visible(document.querySelector('.titlebar .search-box')),
    };
    if (!base.sortVisible || !base.filterVisible || !base.searchBtnVisible) return base;

    // 排序按钮：点出排序菜单（含排序项），点遮罩关闭
    sortBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await raf2();
    const menu = document.querySelector('.mask .menu');
    base.sortMenuOpened = !!menu;
    base.hasSortItem = (menu?.textContent ?? '').includes('修改时间');
    document.querySelector('.mask')?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await raf2();

    // 筛选按钮：开关筛选工具列（开 → 关还原）
    filterBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await raf2();
    base.filterBarOpened = !!document.querySelector('.filterbar');
    filterBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await raf2();

    // 搜索浮层：按钮点开 → 输入框出现并聚焦 → 点遮罩关闭
    searchBtn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await raf2();
    const input = document.querySelector('.mobile-search-mask input');
    base.searchOverlayOpened = !!input;
    base.searchInputFocused = document.activeElement === input;
    document.querySelector('.mobile-search-mask')?.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await raf2();
    base.searchOverlayClosed = !document.querySelector('.mobile-search-mask');
    return base;
  } catch (e) {
    return { error: true, message: String(e) };
  }
})()`;

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
  const tick = async () => {
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
    const titlebar = await win.webContents.executeJavaScript(TITLEBAR_SRC).catch(() => null);
    emit({ type: 'titlebar', t: Date.now() - t0, ...(titlebar ?? { error: true }) });
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
    // 预览浮层打开依赖图片加载，固定 sleep 在慢机上会抖动：轮询等浮层出现（上限 8s）再断言
    const previewDeadline = Date.now();
    for (;;) {
      const opened = await win.webContents
        .executeJavaScript(`!!document.querySelector('.overlay')`)
        .catch(() => false);
      if (opened || Date.now() - previewDeadline > 8000) break;
      await new Promise((r) => setTimeout(r, 150));
    }
    const v = await win.webContents.executeJavaScript(PREVIEW_SRC).catch(() => null);
    emit({ type: 'preview', t: Date.now() - t0, ...(v ?? { error: true }) });
    await shot('preview');

    // 写路径回旧：上传 → SSE → 卡片出现；删除 → 卡片消失。覆盖两类历史回归：
    // （1）item.updated（同内容复活）在未过滤视图不重拉骨架 → 上传后卡片不出现；
    // （2）多路径 item 卡片级删除只回收一个位置 → 删除后卡片残留。探针为 admin token，可写
    const UPDOWN_SRC = `(() => ({
      cards: document.querySelectorAll('.card').length,
    }))()`;
    const before = await win.webContents.executeJavaScript(UPDOWN_SRC).catch(() => null);
    const upload = await win.webContents.executeJavaScript(`(async () => {
      const token = new URLSearchParams(location.search).get('token');
      const blob = new Blob(['smoke-write-' + Date.now()], { type: 'image/png' });
      const form = new FormData();
      form.append('file', blob, 'smoke-write.png');
      const res = await fetch('/api/v1/item/upload', { method: 'POST', body: form, headers: { Authorization: 'Bearer ' + token } });
      const json = await res.json();
      return { status: res.status, id: json?.data?.item?.id, existed: json?.data?.already_existed };
    })()`).catch((e) => ({ error: String(e) }));
    let upCount = before?.cards ?? -1;
    for (let i = 0; i < 20; i++) {
      await new Promise((r) => setTimeout(r, 300));
      upCount = (await win.webContents.executeJavaScript(UPDOWN_SRC).catch(() => null))?.cards ?? -1;
      if (upCount === (before?.cards ?? 0) + 1) break;
    }
    let delStatus = 0;
    if (upload?.id) {
      delStatus = await win.webContents
        .executeJavaScript(
          `(async () => {
      const token = new URLSearchParams(location.search).get('token');
      const res = await fetch('/api/v1/item/delete', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + token },
        body: JSON.stringify({ id: ${JSON.stringify(upload.id)} }),
      });
      return res.status;
    })()`,
        )
        .catch(() => 0);
    }
    let delCount = upCount;
    for (let i = 0; i < 20; i++) {
      await new Promise((r) => setTimeout(r, 300));
      delCount = (await win.webContents.executeJavaScript(UPDOWN_SRC).catch(() => null))?.cards ?? -1;
      if (delCount === (before?.cards ?? 0)) break;
    }
    emit({
      type: 'write-flow',
      t: Date.now() - t0,
      before: before?.cards,
      uploadStatus: upload?.status,
      cardsAfterUpload: upCount,
      deleteStatus: delStatus,
      cardsAfterDelete: delCount,
    });
    app.exit(0);
  };
  // 首个 tick 立即执行：页面可能在 400ms 间隔内就完成 starting→ready（本机回环 + 小库），
  // 只靠 setInterval 会漏检启动屏（assertion 偶发失败的真实原因）
  void tick();
  const timer = setInterval(tick, 400);
});

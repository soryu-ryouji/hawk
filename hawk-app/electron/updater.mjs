// 应用更新（GitHub Releases：stable = latest 正式版比 semver；nightly = 滚动预发布比构建 sha）。
// 检查/比较/下载/校验在本进程完成（无自建服务），安装由平台接力：
// Windows hawk-update.exe 辅助程序 / macOS detached sh 脚本 / Linux AppImage 原地替换 + relaunch。
import { app, ipcMain } from 'electron';
import { spawn } from 'node:child_process';
import path from 'node:path';
import fs from 'node:fs';
import crypto from 'node:crypto';
import { ELECTRON_DIR } from './paths.mjs';
import { getMainWindow, setQuitting } from './window.mjs';

const UPDATE_REPO = 'soryu-ryouji/hawk';
/** 上次检查发现且未安装的更新（下载/安装操作的对象；会话级状态，渲染层重启不丢失） */
let pendingUpdate = null;
/** 已下载并校验通过的更新包路径 */
let verifiedFile = null;

/** 本机构建标识（build-info.json 随包分发，打包前由 scripts/stamp-build.mjs 写入；dev 无文件时 sha='dev'） */
function readBuildInfo() {
  try {
    return JSON.parse(fs.readFileSync(path.join(ELECTRON_DIR, '..', 'build-info.json'), 'utf8'));
  } catch {
    return { sha: 'dev' };
  }
}

/** 按平台选 Release 资产（名称约定见 electron-builder.yml artifactName 与 release.yml：
 *  统一「产品-平台-架构」后缀，mac 由 CI 打包命名） */
function pickAsset(assets) {
  const want =
    process.platform === 'win32'
      ? 'hawk-windows-x64.zip'
      : process.platform === 'darwin'
        ? `hawk-mac-${process.arch === 'arm64' ? 'arm64' : 'x64'}.zip`
        : 'hawk-linux-x64.AppImage';
  return (assets ?? []).find((a) => a.name === want) ?? null;
}

/** 解析 `v1.2.3` 形式的 semver；无法解析返回 null */
function parseSemver(tag) {
  const m = /^v?(\d+)\.(\d+)\.(\d+)/.exec(String(tag));
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
}

/** remote 是否比 local 新（major→minor→patch 逐位，首位不等即分出高低；local 缺失视为全新） */
function semverNewer(remote, local) {
  if (!local) {
    return true;
  }
  for (let i = 0; i < 3; i++) {
    const r = remote[i] ?? 0;
    const l = local[i] ?? 0;
    if (r !== l) {
      return r > l;
    }
  }
  return false;
}

async function fetchRelease(channel) {
  const url =
    channel === 'stable'
      ? `https://api.github.com/repos/${UPDATE_REPO}/releases/latest`
      : `https://api.github.com/repos/${UPDATE_REPO}/releases/tags/nightly`;
  const res = await fetch(url, {
    headers: { accept: 'application/vnd.github+json', 'user-agent': 'hawk-app' },
    signal: AbortSignal.timeout(15000),
  });
  if (res.status === 404) {
    throw new Error(channel === 'stable' ? '暂无稳定版发布' : '暂无 nightly 发布');
  }
  if (!res.ok) {
    throw new Error(`GitHub API 请求失败（HTTP ${res.status}）`);
  }
  return res.json();
}

export function registerUpdaterIpc() {
  ipcMain.handle('hawk:app-version', () => ({ version: app.getVersion(), sha: readBuildInfo().sha }));

  ipcMain.handle('hawk:update-check', async (_event, channel) => {
    if (channel !== 'stable' && channel !== 'nightly') {
      throw new Error('未知更新通道');
    }
    const release = await fetchRelease(channel);
    const asset = pickAsset(release.assets);
    if (!asset) {
      throw new Error('当前平台暂无更新包');
    }
    let version;
    let available;
    if (channel === 'stable') {
      // stable：tag v* 与 app.getVersion() 比 semver（发版时需同步 bump package.json）
      const remote = parseSemver(release.tag_name);
      if (!remote) {
        throw new Error(`无法解析版本号：${release.tag_name}`);
      }
      const local = parseSemver(app.getVersion());
      available = semverNewer(remote, local);
      version = release.tag_name.replace(/^v/, '');
    } else {
      // nightly：滚动 tag 固定，比 Release 所指 commit 与本机构建 sha。
      // Release 的 target_commitish 是分支名不是 sha：CI 在 body 末尾注入 hawk-nightly-sha 注释；
      // 注释机制上线前的旧 nightly 退化为 Release 名（Nightly <短sha>）前缀匹配
      const localSha = readBuildInfo().sha;
      if (localSha === 'dev') {
        throw new Error('开发构建无构建标识，无法检查 nightly 更新');
      }
      const bodySha = /hawk-nightly-sha:\s*([0-9a-f]{7,40})/i.exec(release.body || '');
      const nameSha = /nightly\s+([0-9a-f]{7,40})/i.exec(release.name || '');
      const remote = bodySha ?? nameSha;
      if (!remote) {
        throw new Error('nightly 发布缺少构建标识，无法比较');
      }
      // 短 sha（Release 名）与本机全 sha 按短侧长度前缀比较
      const n = Math.min(remote[1].length, localSha.length);
      available = remote[1].slice(0, n).toLowerCase() !== localSha.slice(0, n).toLowerCase();
      version = remote[1].slice(0, 7).toLowerCase();
    }
    if (!available) {
      pendingUpdate = null;
      verifiedFile = null;
      return null;
    }
    pendingUpdate = { channel, version, asset };
    verifiedFile = null;
    return {
      channel,
      version,
      notes: release.body || '',
      url: release.html_url,
      assetName: asset.name,
      size: asset.size ?? 0,
    };
  });

  /** 下载上次检查到的更新包并强制 sha256 校验（边车 <artifact>.sha256 缺失即失败，不提供无校验的更新）。
   *  进度经 hawk:update-progress 事件推送，完成后 resolve */
  ipcMain.handle('hawk:update-download', async () => {
    if (!pendingUpdate) {
      throw new Error('请先检查更新');
    }
    if (verifiedFile && fs.existsSync(verifiedFile)) {
      return; // 已就绪，重复点击幂等
    }
    const dir = path.join(app.getPath('temp'), 'hawk-update');
    fs.mkdirSync(dir, { recursive: true });
    const file = path.join(dir, pendingUpdate.asset.name);
    const sendProgress = (p) => getMainWindow()?.webContents.send('hawk:update-progress', p);
    sendProgress({ phase: 'downloading', received: 0, total: pendingUpdate.asset.size ?? 0 });
    const res = await fetch(pendingUpdate.asset.browser_download_url, { headers: { 'user-agent': 'hawk-app' } });
    if (!res.ok) {
      throw new Error(`下载失败（HTTP ${res.status}）`);
    }
    const total = Number(res.headers.get('content-length')) || pendingUpdate.asset.size || 0;
    const reader = res.body.getReader();
    const out = fs.openSync(file, 'w');
    let received = 0;
    let lastPct = -1;
    let lastSent = 0;
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) {
          break;
        }
        fs.writeSync(out, value);
        received += value.byteLength;
        // 节流：百分比变化或每 256KB 发一次，避免大包刷屏 IPC
        const pct = total > 0 ? Math.floor((received * 100) / total) : -1;
        if (pct !== lastPct || received - lastSent >= 262144) {
          lastPct = pct;
          lastSent = received;
          sendProgress({ phase: 'downloading', received, total });
        }
      }
    } finally {
      fs.closeSync(out);
    }
    sendProgress({ phase: 'verifying' });
    const sumRes = await fetch(`${pendingUpdate.asset.browser_download_url}.sha256`, { headers: { 'user-agent': 'hawk-app' } });
    if (!sumRes.ok) {
      throw new Error('发布包缺少 sha256 校验文件，请到 GitHub 手动下载更新');
    }
    const expected = (await sumRes.text()).trim().toLowerCase();
    const actual = crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
    if (actual !== expected) {
      throw new Error('更新包校验失败（sha256 不匹配）');
    }
    verifiedFile = file;
    sendProgress({ phase: 'ready' });
  });

  /** 重启并安装已校验的更新：成功后本进程退出（IPC 不再返回），由更新辅助程序接力（Windows）
   *  或平台替换脚本接力（macOS/Linux） */
  ipcMain.handle('hawk:update-install', () => {
    if (!verifiedFile || !fs.existsSync(verifiedFile)) {
      throw new Error('更新包尚未就绪');
    }
    setQuitting(); // 放行 close 拦截，app.quit 走真正退出路径（will-quit 回收 server）
    if (process.platform === 'linux') {
      // AppImage：拷贝到同目录后原子改名覆盖自身（运行中的旧挂载来自旧 inode 不受影响），relaunch 重启
      const staged = `${process.execPath}.update`;
      fs.copyFileSync(verifiedFile, staged);
      fs.chmodSync(staged, 0o755);
      fs.renameSync(staged, process.execPath);
      app.relaunch();
    } else if (process.platform === 'darwin') {
      installMacUpdate(verifiedFile);
    } else {
      installWindowsUpdate(verifiedFile);
    }
    app.quit();
  });
}

/** shell 单引号转义（路径含空格/特殊字符时保持一个参数） */
function shQuote(s) {
  return `'${String(s).replace(/'/g, `'\\''`)}'`;
}

/** macOS：detached sh 脚本等旧进程退出 → 解压 zip → 替换 .app → 拉起新实例。
 *  解压/暂存目录与 .app 同目录（同卷，mv 原子）；app 内 fetch 下载无 quarantine 标记，不触发 Gatekeeper */
function installMacUpdate(zip) {
  const bundle = path.dirname(path.dirname(path.dirname(process.execPath))); // hawk.app
  const parent = path.dirname(bundle);
  const staging = path.join(parent, '.hawk-update');
  const script = path.join(parent, '.hawk-update.sh');
  fs.writeFileSync(
    script,
    `#!/bin/sh
while kill -0 ${process.pid} 2>/dev/null; do sleep 0.3; done
rm -rf ${shQuote(staging)}
ditto -x -k ${shQuote(zip)} ${shQuote(staging)}
rm -rf ${shQuote(bundle)}
mv ${shQuote(path.join(staging, 'hawk.app'))} ${shQuote(bundle)}
rm -rf ${shQuote(staging)} ${shQuote(zip)} ${shQuote(script)}
open ${shQuote(bundle)}
`,
  );
  fs.chmodSync(script, 0o755);
  spawn('/bin/sh', [script], { detached: true, stdio: 'ignore' }).unref();
}

/** Windows 绿色版更新：复制辅助程序（resources/hawk-update/hawk-update.exe，实现见仓库根
 *  hawk-update/）到更新临时目录后启动，由它等旧进程退出 → 解压 zip → 覆盖应用目录 → 拉起新实例。
 *  temp 副本运行：更新会覆盖应用目录内的 hawk-update.exe，运行中的自身无法被覆盖。
 *  全过程写更新目录 install.log，失败非零退出——不静默，留现场 */
function installWindowsUpdate(zip) {
  const appDir = path.dirname(process.execPath);
  const dir = path.dirname(zip);
  const updaterSrc = path.join(process.resourcesPath, 'hawk-update', 'hawk-update.exe');
  if (!fs.existsSync(updaterSrc)) {
    throw new Error('更新辅助程序缺失，请到 GitHub 手动下载更新');
  }
  // 同名旧副本是上次运行残留；删除失败（被占用）则改用带 pid 的名字
  let runCopy = path.join(dir, 'hawk-update.exe');
  try {
    fs.rmSync(runCopy, { force: true });
  } catch {
    runCopy = path.join(dir, `hawk-update-${process.pid}.exe`);
  }
  fs.copyFileSync(updaterSrc, runCopy);
  spawn(runCopy, ['--pid', String(process.pid), '--zip', zip, '--app', appDir], {
    detached: true,
    stdio: 'ignore',
    windowsHide: true,
  }).unref();
}

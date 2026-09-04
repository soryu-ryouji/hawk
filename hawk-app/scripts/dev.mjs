// 一键开发：esbuild 首轮构建主进程/preload → 启动 vite → 等 5173 就绪后拉起 electron
// （server 由 electron 主进程拉起）。electron/src 改动 → 自动重建 → 自动重启 electron。
import * as esbuild from 'esbuild';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { ELECTRON_BUILD_COMMON, ELECTRON_BUILDS } from './electron-build-config.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(import.meta.url);

// 主进程/preload 的 esbuild context（首轮构建 + watch 重建共用）
const contexts = await Promise.all(
  ELECTRON_BUILDS.map((c) => esbuild.context({ ...ELECTRON_BUILD_COMMON, ...c, absWorkingDir: root })),
);
await Promise.all(contexts.map((ctx) => ctx.rebuild()));

const vite = spawn('npm', ['run', 'dev:web'], { cwd: root, stdio: 'inherit', shell: process.platform === 'win32' });

const deadline = Date.now() + 60_000;
for (;;) {
  try {
    const res = await fetch('http://localhost:5173/');
    if (res.ok || res.status === 404) break;
  } catch { /* vite 未就绪 */ }
  if (Date.now() > deadline) {
    console.error('vite 启动超时');
    vite.kill();
    process.exit(1);
  }
  await new Promise((r) => setTimeout(r, 300));
}

const electronBin = require('electron');

let app = null;
/** 为重启而杀的标志：electron exit 时据此区分「重建重启」与「用户退出」（托盘退出/Cmd+Q） */
let restarting = false;

function spawnApp() {
  app = spawn(electronBin, ['.'], { cwd: root, stdio: 'inherit', env: process.env });
  app.on('exit', (code) => {
    if (restarting) {
      restarting = false;
      spawnApp();
      return;
    }
    vite.kill();
    void Promise.all(contexts.map((ctx) => ctx.dispose()));
    process.exit(code ?? 0);
  });
}

spawnApp();

// electron/src 改动 → 防抖重建 → 重启 electron（首轮 spawnApp 前的重建不重启：app 尚未拉起）
let rebuildTimer;
fs.watch(path.join(root, 'electron', 'src'), { recursive: true }, () => {
  clearTimeout(rebuildTimer);
  rebuildTimer = setTimeout(async () => {
    try {
      await Promise.all(contexts.map((ctx) => ctx.rebuild()));
    } catch (e) {
      // 重建失败（编辑中间态的语法错误等）：不重启，等下一次改动
      console.error(`[dev] 主进程/preload 重建失败: ${e instanceof Error ? e.message : e}`);
      return;
    }
    if (!app || app.killed) {
      return;
    }
    console.log('[dev] 主进程/preload 已重建，重启 electron…');
    restarting = true;
    app.kill();
  }, 200);
});

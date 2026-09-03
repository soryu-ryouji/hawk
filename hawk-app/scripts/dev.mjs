// 一键开发：先构建主进程/preload（esbuild watch 持续重建），启动 vite，等 5173 就绪后拉起 electron
// （server 由 electron 主进程拉起）。主进程 TS 改动由 watch 重建，重开 electron 生效。
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(import.meta.url);

// 主进程/preload 构建（watch 模式后台常驻；electron 退出时随本进程一起回收）
const electronBuild = spawn('node', ['scripts/build-electron.mjs', '--watch'], { cwd: root, stdio: 'inherit' });

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

// 等 esbuild 首轮产物落盘（watch 模式 rebuild 完成即开始监听，产物存在即可拉起 electron）
{
  const fs = await import('node:fs');
  const mainOut = path.join(root, 'electron', 'out', 'main.mjs');
  const buildDeadline = Date.now() + 30_000;
  while (!fs.existsSync(mainOut)) {
    if (Date.now() > buildDeadline) {
      console.error('electron 主进程构建超时（electron/out/main.mjs 未生成）');
      vite.kill();
      electronBuild.kill();
      process.exit(1);
    }
    await new Promise((r) => setTimeout(r, 100));
  }
}

const electronBin = require('electron');
const app = spawn(electronBin, ['.'], { cwd: root, stdio: 'inherit', env: process.env });

app.on('exit', (code) => {
  vite.kill();
  electronBuild.kill();
  process.exit(code ?? 0);
});

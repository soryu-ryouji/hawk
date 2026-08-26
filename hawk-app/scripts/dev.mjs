// 一键开发：启动 vite，等 5173 就绪后拉起 electron（server 由 electron 主进程拉起）。
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(import.meta.url);

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
const app = spawn(electronBin, ['.'], { cwd: root, stdio: 'inherit', env: process.env });

app.on('exit', (code) => {
  vite.kill();
  process.exit(code ?? 0);
});

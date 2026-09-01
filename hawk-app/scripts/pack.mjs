// electron-builder 包装：默认降低 7z 压缩级别换取打包速度（mx=9 单线程 2 分钟 → mx=5 约 1 分钟）。
// 覆盖：ELECTRON_BUILDER_COMPRESSION_LEVEL=9 npm run pack（最小体积）；=3 约 20 秒（+27MB）。
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { stampBuildInfo } from './stamp-build.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// 先写入构建标识（electron-builder 的 files 需要它存在），再进 electron-builder
stampBuildInfo();

process.env.ELECTRON_BUILDER_COMPRESSION_LEVEL ??= '5';

const cli = path.join(root, 'node_modules', 'electron-builder', 'cli.js');
const result = spawnSync(process.execPath, [cli, ...process.argv.slice(2)], {
  cwd: root,
  stdio: 'inherit',
  env: process.env,
});
process.exit(result.status ?? 1);

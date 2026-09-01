// 构建标识：打包前写入 hawk-app/build-info.json（随 asar 分发，主进程自动更新比较 nightly 新旧用）。
// sha 优先取 HAWK_SHA 环境变量（CI 注入），否则本机 git HEAD，再退回 'dev'（无 git 环境：nightly 通道无法比较）。
// 被 pack.mjs 导入；CI 中不经 pack.mjs 的路径（macos 双架构直接调 electron-builder）以
// `node scripts/stamp-build.mjs` 直接执行。
import { execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/** 写入 build-info.json；返回写入的 sha */
export function stampBuildInfo() {
  let sha = process.env.HAWK_SHA || '';
  if (!sha) {
    try {
      sha = execSync('git rev-parse HEAD', { cwd: root, stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim();
    } catch {
      sha = 'dev';
    }
  }
  fs.writeFileSync(path.join(root, 'build-info.json'), JSON.stringify({ sha }, null, 2));
  return sha;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log(`build-info.json stamped: ${stampBuildInfo()}`);
}

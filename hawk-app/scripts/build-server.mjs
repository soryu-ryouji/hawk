// 发布当前平台的 hawk-server 自包含单文件到 resources/hawk-server/（electron-builder 的 extraResources 来源）。
// 用法：node scripts/build-server.mjs [RID]   例：node scripts/build-server.mjs osx-arm64（交叉编译）
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const RIDS = {
  'win32-x64': 'win-x64',
  'darwin-arm64': 'osx-arm64',
  'darwin-x64': 'osx-x64',
  'linux-x64': 'linux-x64',
};

const rid = process.argv[2] ?? RIDS[`${process.platform}-${process.arch}`];
if (!rid) {
  console.error(`未知平台 ${process.platform}-${process.arch}，请显式传入 RID`);
  process.exit(1);
}

const out = path.join(root, 'resources', 'hawk-server');
fs.rmSync(out, { recursive: true, force: true });

const result = spawnSync('dotnet', [
  'publish', path.join(root, '..', 'hawk-server', 'hawk-server.csproj'),
  '-c', 'Release', '-r', rid, '--self-contained',
  // 不开启 EnableCompressionInSingleFile：portable 打包时 NSIS 还会整体压缩，预压缩反而使最终 exe 变大
  '-p:PublishSingleFile=true', '-o', out,
], { stdio: 'inherit' });

if (result.error || result.status !== 0) {
  process.exit(result.status ?? 1);
}
console.log(`已发布 ${rid} → resources/hawk-server`);

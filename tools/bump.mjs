#!/usr/bin/env node
// 版本号 bump：只改 hawk-app/package.json（daemon 的 Go 版本号不再单独维护）。
// CI 发布只用 hawk-app/package.json 的版本号，提交信息不重复写版本。
//
// 用法：node tools/bump.mjs 2.5.0
// 改完提交 `release: v2.5.0`（前缀是发版开关，CI 从 package.json 读版本号）。
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SEMVER = /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;
const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';

function fail(message) {
  console.error(message);
  process.exit(1);
}

const version = process.argv[2];
if (!version) fail('缺少版本号（用法：node tools/bump.mjs <version>）');
if (!SEMVER.test(version)) fail(`版本号不是 semver: ${version}`);

const dir = path.join(root, 'hawk-app');
const before = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8')).version;
if (before === version) {
  console.log('版本号未变化，没有改动');
  process.exit(0);
}

execFileSync(npm, ['version', version, '--no-git-tag-version', '--allow-same-version'], {
  cwd: dir,
  stdio: 'inherit',
  // Node 22+ 在 Windows 上不再允许直接 spawn .cmd/.bat，npm.cmd 需经 shell 启动
  shell: process.platform === 'win32',
});

console.log(`已更新：\n  hawk-app/package.json  ${before} → ${version}`);
console.log(`\n提交：git commit -am "release: v${version}"（CI 从 package.json 读版本号）`);

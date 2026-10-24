#!/usr/bin/env node
// 版本号 bump：一条命令改完 package.json 与 Cargo.toml（Cargo.lock 顺带刷新），
// 不用手工对齐多处版本号。CI 发布只用 hawk-app/package.json 的版本号，提交信息不重复写版本。
//
// 用法：
//   node tools/bump.mjs 2.5.0                   # 只改 hawk-app（发版最常用）
//   node tools/bump.mjs 2.5.0 --daemon 0.5.0    # 同时改 hawk-daemon
//   node tools/bump.mjs --server 0.4.0          # 只改 hawk-server
//
// 改完提交 `release: v2.5.0`（前缀是发版开关，CI 从 package.json 读版本号）。
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SEMVER = /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;
const CRATES = { daemon: 'hawk-daemon', server: 'hawk-server' };
const npm = process.platform === 'win32' ? 'npm.cmd' : 'npm';

function fail(message) {
  console.error(message);
  process.exit(1);
}

const want = {};
const args = process.argv.slice(2);
for (let i = 0; i < args.length; i++) {
  const arg = args[i];
  if (arg === '--app' || arg === '--daemon' || arg === '--server') {
    const version = args[++i];
    if (!version) fail(`${arg} 缺少版本号`);
    want[arg.slice(2)] = version;
  } else if (arg.startsWith('--')) {
    fail(`未知参数: ${arg}`);
  } else if (want.app) {
    fail(`多余的参数: ${arg}`);
  } else {
    want.app = arg;
  }
}

if (Object.keys(want).length === 0) fail('缺少版本号（用法见 tools/bump.mjs 文件头）');
for (const [name, version] of Object.entries(want)) {
  if (!SEMVER.test(version)) fail(`${name} 版本号不是 semver: ${version}`);
}

const changes = [];

if (want.app) {
  const dir = path.join(root, 'hawk-app');
  const before = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8')).version;
  if (before !== want.app) {
    // npm 自己改 package.json + package-lock.json（两处 version 都要跟着走，手改容易漏）
    execFileSync(npm, ['version', want.app, '--no-git-tag-version', '--allow-same-version'], {
      cwd: dir,
      stdio: 'inherit',
      // Node 22+ 在 Windows 上不再允许直接 spawn .cmd/.bat，npm.cmd 需经 shell 启动
      shell: process.platform === 'win32',
    });
    changes.push(`hawk-app/package.json  ${before} → ${want.app}`);
  }
}

for (const [name, crate] of Object.entries(CRATES)) {
  if (!want[name]) continue;
  const file = path.join(root, crate, 'Cargo.toml');
  const text = fs.readFileSync(file, 'utf8');
  // [package] 段在文件开头，version = "..." 独立成行；内联的依赖版本号写作 `name = { version = ... }`，不会命中
  const matched = text.match(/^version = "([^"]+)"/m);
  const before = matched?.[1];
  if (!before) fail(`${crate}/Cargo.toml 里没找到 [package] 的 version 行`);
  if (before === want[name]) continue;
  fs.writeFileSync(file, text.replace(/^version = "[^"]+"/m, `version = "${want[name]}"`));
  changes.push(`${crate}/Cargo.toml  ${before} → ${want[name]}`);
}

// crate 版本变了要刷 Cargo.lock（server 的 lock 里也记着 daemon 的版本，两个都跑一遍）
if (want.daemon || want.server) {
  for (const crate of Object.values(CRATES)) {
    try {
      execFileSync('cargo', ['metadata', '--format-version', '1'], {
        cwd: path.join(root, crate),
        stdio: 'ignore',
      });
    } catch {
      console.warn(`警告: cargo metadata 失败（${crate}），Cargo.lock 会在下次构建时自动刷新`);
    }
  }
}

if (changes.length === 0) {
  console.log('版本号未变化，没有改动');
  process.exit(0);
}

console.log('已更新：');
for (const line of changes) console.log(`  ${line}`);
if (want.app) console.log(`\n提交：git commit -am "release: v${want.app}"（CI 从 package.json 读版本号）`);

// 发布当前平台的 hawk-daemon（Rust，cargo build --release）到 resources/hawk-daemon/（electron-builder 的 extraResources 来源）。
// 用法：node scripts/build-server.mjs [RID]   例：node scripts/build-server.mjs osx-arm64（mac 同机交叉到 arm64）
// RID 为沿用名：win-x64 / osx-arm64 / osx-x64 / linux-x64，内部映射到 rust target triple。
// 交叉 target 自动 rustup target add；本机已有 target 时 incremental 构建很快。
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const RIDS = {
  'win32-x64': 'x86_64-pc-windows-msvc',
  'darwin-arm64': 'aarch64-apple-darwin',
  'darwin-x64': 'x86_64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu',
};

// 沿用名 → rust target（也接受直接传 target triple）
const ALIASES = {
  'win-x64': 'x86_64-pc-windows-msvc',
  'osx-arm64': 'aarch64-apple-darwin',
  'osx-x64': 'x86_64-apple-darwin',
  'linux-x64': 'x86_64-unknown-linux-gnu',
};

const arg = process.argv[2] ?? RIDS[`${process.platform}-${process.arch}`];
const target = ALIASES[arg] ?? arg;
if (!target) {
  console.error(`未知平台 ${process.platform}-${process.arch}，请显式传入 RID`);
  process.exit(1);
}

// 非本机 target：先安装交叉编译目标（mac runner 同机产出 arm64 + x64；C 依赖 sqlite/libwebp 由 cc 按 -arch 编译）
const native = RIDS[`${process.platform}-${process.arch}`];
if (target !== native) {
  const add = spawnSync('rustup', ['target', 'add', target], { stdio: 'inherit' });
  if (add.status !== 0) process.exit(add.status ?? 1);
}

const rsDir = path.join(root, '..', 'hawk-daemon');
const result = spawnSync(
  'cargo',
  ['build', '--release', '--manifest-path', path.join(rsDir, 'Cargo.toml'), '--target', target],
  { stdio: 'inherit' },
);
if (result.error || result.status !== 0) {
  process.exit(result.status ?? 1);
}

const exe = target.includes('windows') ? 'hawk-daemon.exe' : 'hawk-daemon';
const built = path.join(rsDir, 'target', target, 'release', exe);
const out = path.join(root, 'resources', 'hawk-daemon');
fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });
fs.copyFileSync(built, path.join(out, exe));
console.log(`已发布 ${target} → resources/hawk-daemon`);

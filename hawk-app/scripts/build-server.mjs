// 发布当前平台的 hawk-daemon（Go）到 resources/hawk-daemon/（electron-builder 的 extraResources 来源）。
// 用法：node scripts/build-server.mjs [RID]   例：node scripts/build-server.mjs osx-arm64
// RID：win-x64 / osx-arm64 / osx-x64 / linux-x64；内部映射到 GOOS/GOARCH。
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const daemonDir = path.join(root, '..', 'hawk-daemon');

const RIDS = {
  'win32-x64': { goos: 'windows', goarch: 'amd64', rid: 'win-x64' },
  'darwin-arm64': { goos: 'darwin', goarch: 'arm64', rid: 'osx-arm64' },
  'darwin-x64': { goos: 'darwin', goarch: 'amd64', rid: 'osx-x64' },
  'linux-x64': { goos: 'linux', goarch: 'amd64', rid: 'linux-x64' },
};

const ALIASES = {
  'win-x64': { goos: 'windows', goarch: 'amd64' },
  'osx-arm64': { goos: 'darwin', goarch: 'arm64' },
  'osx-x64': { goos: 'darwin', goarch: 'amd64' },
  'linux-x64': { goos: 'linux', goarch: 'amd64' },
};

const arg = process.argv[2] ?? RIDS[`${process.platform}-${process.arch}`]?.rid;
const target = ALIASES[arg] ?? RIDS[`${process.platform}-${process.arch}`];
if (!target) {
  console.error(`未知平台 ${process.platform}-${process.arch}，请显式传入 RID`);
  process.exit(1);
}

const ext = target.goos === 'windows' ? '.exe' : '';
const out = path.join(root, 'resources', 'hawk-daemon');
fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });
const built = path.join(out, `hawk-daemon${ext}`);

const result = spawnSync('go', ['build', '-trimpath', '-ldflags', '-s -w', '-o', built, '.'], {
  cwd: daemonDir,
  stdio: 'inherit',
  env: { ...process.env, CGO_ENABLED: '0', GOOS: target.goos, GOARCH: target.goarch },
});
if (result.error || result.status !== 0) {
  process.exit(result.status ?? 1);
}
console.log(`已发布 ${target.goos}/${target.goarch} → resources/hawk-daemon`);

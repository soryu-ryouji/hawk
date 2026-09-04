// 从运行中的 hawk-daemon（Rust 版）拉取 OpenAPI schema，生成 TS 类型到 web/src/api/schema.d.ts。
// 用法：npm run gen:types（需先构建 Rust 后端：cargo build --release，或 debug 亦可）
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const tmp = path.join(root, 'tools', '.tmp');
const lib = path.join(tmp, 'gen-types-lib');
const port = 27397;
const exe = process.platform === 'win32' ? 'hawk-daemon.exe' : 'hawk-daemon';
const RUST_TARGET = { 'win32-x64': 'x86_64-pc-windows-msvc', 'darwin-arm64': 'aarch64-apple-darwin', 'darwin-x64': 'x86_64-apple-darwin', 'linux-x64': 'x86_64-unknown-linux-gnu' }[`${process.platform}-${process.arch}`];
const candidates = [
  // 本机直建与 --target 交叉建两种产物位置
  ...(RUST_TARGET ? [path.join(root, '..', 'hawk-daemon', 'target', RUST_TARGET, 'release', exe)] : []),
  path.join(root, '..', 'hawk-daemon', 'target', 'release', exe),
  path.join(root, '..', 'hawk-daemon', 'target', 'debug', exe),
];
// 本机直建与 --target 交叉建两种产物位置可能同时存在（版本不一），取 mtime 最新者——
// 固定优先级会在交叉产物过期时静默用旧 schema 生成类型
const bin = candidates
  .filter((p) => fs.existsSync(p))
  .sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs)[0];
if (!bin) {
  console.error('未找到 hawk-daemon 构建产物，请先 cargo build（release 或 debug）');
  process.exit(1);
}

fs.rmSync(tmp, { recursive: true, force: true });
fs.mkdirSync(lib, { recursive: true });

const server = spawn(bin, ['--library', lib, '--port', String(port)], {
  env: { ...process.env, HAWK_TOKEN: 'gen-types' },
  stdio: 'ignore',
});

try {
  const base = `http://127.0.0.1:${port}`;
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      const res = await fetch(`${base}/health`);
      if (res.ok) break;
    } catch { /* 未就绪 */ }
    if (Date.now() > deadline) throw new Error('hawk-daemon 启动超时');
    await new Promise((r) => setTimeout(r, 300));
  }

  const schema = await (await fetch(`${base}/openapi/v1.json`)).json();
  const schemaFile = path.join(tmp, 'openapi.json');
  fs.writeFileSync(schemaFile, JSON.stringify(schema, null, 2));

  // .bin 下的 shim 在 Windows 上是 shell 脚本，直接定位包内真实 CLI 文件，绕开 exports 条件映射
  const cli = path.join(root, 'node_modules', 'openapi-typescript', 'bin', 'cli.js');
  const out = path.join(root, 'web', 'src', 'api', 'schema.d.ts');
  execFileSync(process.execPath, [cli, schemaFile, '-o', out]);
  console.log(`已生成 ${out}`);
} finally {
  server.kill();
  fs.rmSync(tmp, { recursive: true, force: true });
}

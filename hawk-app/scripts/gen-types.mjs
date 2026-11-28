// 从运行中的 hawk-daemon（Go 版）拉取 OpenAPI schema，生成 TS 类型到 web/src/shared/api/schema.d.ts。
// 用法：npm run gen:types（需先在 hawk-daemon/ 执行 go build -o hawk-daemon.exe .）
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const tmp = path.join(root, 'tools', '.tmp');
const lib = path.join(tmp, 'gen-types-lib');
const identity = path.join(tmp, 'gen-types-identity');
const cache = path.join(tmp, 'gen-types-cache');
const port = 27397;
const exe = process.platform === 'win32' ? 'hawk-daemon.exe' : 'hawk-daemon';
const candidates = [
  path.join(root, '..', 'hawk-daemon', exe),
  path.join(root, '..', 'hawk-daemon', 'bin', exe),
  path.join(root, 'resources', 'hawk-daemon', exe),
];
const bin = candidates.filter((p) => fs.existsSync(p))[0];
if (!bin) {
  console.error('未找到 hawk-daemon 构建产物，请先在 hawk-daemon/ 执行 go build -o hawk-daemon.exe .');
  process.exit(1);
}

fs.rmSync(tmp, { recursive: true, force: true });
fs.mkdirSync(lib, { recursive: true });
fs.mkdirSync(identity, { recursive: true });

const server = spawn(bin, ['local', lib, '--port', String(port), '--identity-dir', identity, '--cache-parent', cache], {
  env: { ...process.env, HAWK_ADMIN_TOKEN: 'gen-types' },
  stdio: 'ignore',
});

try {
  const base = `http://127.0.0.1:${port}`;
  const deadline = Date.now() + 60_000;
  for (;;) {
    try {
      const res = await fetch(`${base}/health`);
      if (res.ok) break;
    } catch {
      /* 未就绪 */
    }
    if (Date.now() > deadline) throw new Error('hawk-daemon 启动超时');
    await new Promise((r) => setTimeout(r, 300));
  }

  const schema = await (await fetch(`${base}/openapi/v1.json`)).json();
  const schemaFile = path.join(tmp, 'openapi.json');
  fs.writeFileSync(schemaFile, JSON.stringify(schema, null, 2));

  // .bin 下的 shim 在 Windows 上是 shell 脚本，直接定位包内真实 CLI 文件，绕开 exports 条件映射
  const cli = path.join(root, 'node_modules', 'openapi-typescript', 'bin', 'cli.js');
  const out = path.join(root, 'web', 'src', 'shared', 'api', 'schema.d.ts');
  execFileSync(process.execPath, [cli, schemaFile, '-o', out]);
  console.log(`已生成 ${out}`);
} finally {
  if (server.exitCode === null && server.signalCode === null) {
    const exited = new Promise((resolve) => server.once('exit', resolve));
    server.kill();
    await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 5000))]);
  }
  fs.rmSync(tmp, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}

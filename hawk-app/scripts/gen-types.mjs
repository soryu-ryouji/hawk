// 从运行中的 hawk-server 拉取 OpenAPI schema，生成 TS 类型到 web/src/api/schema.d.ts。
// 用法：npm run gen:types（需先 dotnet build hawk-server）
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const tmp = path.join(root, 'tools', '.tmp');
const lib = path.join(tmp, 'gen-types-lib');
const port = 27397;
const dll = path.join(root, '..', 'hawk-server', 'bin', 'Debug', 'net10.0', 'hawk-server.dll');

fs.rmSync(tmp, { recursive: true, force: true });
fs.mkdirSync(lib, { recursive: true });

const server = spawn('dotnet', [dll, '--library', lib, '--port', String(port)], {
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
    if (Date.now() > deadline) throw new Error('hawk-server 启动超时');
    await new Promise((r) => setTimeout(r, 300));
  }

  const schema = await (await fetch(`${base}/openapi/v1.json`)).json();
  const schemaFile = path.join(tmp, 'openapi.json');
  fs.writeFileSync(schemaFile, JSON.stringify(schema, null, 2));

  // .bin 下的 shim 在 Windows 上是 shell 脚本，直接定位包内真实 CLI 文件，绕开 exports 条件映射
  const bin = path.join(root, 'node_modules', 'openapi-typescript', 'bin', 'cli.js');
  const out = path.join(root, 'web', 'src', 'api', 'schema.d.ts');
  execFileSync(process.execPath, [bin, schemaFile, '-o', out]);
  console.log(`已生成 ${out}`);
} finally {
  server.kill();
  fs.rmSync(tmp, { recursive: true, force: true });
}

// Electron 主进程/preload 构建：esbuild 打包 electron/src 的 TS 源码到 electron/out/
// （main.mjs ESM 产物 + preload.cjs CJS 产物——sandbox 下 preload 必须为 CJS 单文件）。
// --watch：开发态持续重建（dev.mjs 拉起；electron 进程不自动重启，主进程代码改动手动重开生效）。
import * as esbuild from 'esbuild';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const watch = process.argv.includes('--watch');

const common = {
  absWorkingDir: root,
  bundle: true,
  platform: 'node',
  target: 'node22', // Electron 内嵌 Node 的保守基线（仅影响语法降级，不影响 API 可用性）
  external: ['electron'],
  logLevel: 'warning',
};

const configs = [
  { entryPoints: ['electron/src/main.ts'], outfile: 'electron/out/main.mjs', format: 'esm' },
  { entryPoints: ['electron/src/preload.ts'], outfile: 'electron/out/preload.cjs', format: 'cjs' },
];

const contexts = await Promise.all(configs.map((c) => esbuild.context({ ...common, ...c })));
await Promise.all(contexts.map((ctx) => ctx.rebuild()));
console.log('[build-electron] electron/out 已生成');
if (watch) {
  await Promise.all(contexts.map((ctx) => ctx.watch()));
} else {
  await Promise.all(contexts.map((ctx) => ctx.dispose()));
}

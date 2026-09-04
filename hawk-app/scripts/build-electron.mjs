// Electron 主进程/preload 构建：esbuild 打包 electron/src 的 TS 源码到 electron/out/
// （main.mjs ESM 产物 + preload.cjs CJS 产物——sandbox 下 preload 必须为 CJS 单文件）。
// 一次性构建（pack/CI 用）；开发态的持续重建由 dev.mjs 驱动（见 dev.mjs 的 fs.watch）。
import * as esbuild from 'esbuild';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { ELECTRON_BUILD_COMMON, ELECTRON_BUILDS } from './electron-build-config.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

for (const config of ELECTRON_BUILDS) {
  await esbuild.build({ ...ELECTRON_BUILD_COMMON, ...config, absWorkingDir: root });
}
console.log('[build-electron] electron/out 已生成');

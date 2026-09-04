// Electron 主进程/preload 的 esbuild 构建配置（build-electron.mjs 一次性构建与 dev.mjs watch 重建共用）。
export const ELECTRON_BUILDS = [
  { entryPoints: ['electron/src/main.ts'], outfile: 'electron/out/main.mjs', format: 'esm' },
  { entryPoints: ['electron/src/preload.ts'], outfile: 'electron/out/preload.cjs', format: 'cjs' },
];

export const ELECTRON_BUILD_COMMON = {
  bundle: true,
  platform: 'node',
  target: 'node22', // Electron 内嵌 Node 的保守基线（仅影响语法降级，不影响 API 可用性）
  external: ['electron'],
  logLevel: 'warning',
};

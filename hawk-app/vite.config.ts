/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';
import { fileURLToPath, URL } from 'node:url';

export default defineConfig({
  root: 'web',
  // 公共目录直指 build/（electron-builder 资源目录），icon 等资产保持单一来源
  publicDir: '../build',
  plugins: [vue()],
  // `@` 别名指向 web/src：shared 边界的引用用别名写死，后续域迁移不再改 import
  resolve: { alias: { '@': fileURLToPath(new URL('./web/src', import.meta.url)) } },
  // file:// 打包加载需要相对路径
  base: './',
  server: { port: 5173, strictPort: true },
  build: { outDir: 'dist', emptyOutDir: true },
  test: {
    // 与源码同目录的 *.spec.ts；默认 node 环境（只测纯函数），需要 DOM 的测试用文件头
    // `// @vitest-environment jsdom` 单文件覆盖。electron/src 的主进程纯函数（如 cache-path）同测
    include: ['src/**/*.spec.ts', '../electron/src/**/*.spec.ts'],
    environment: 'node',
  },
});

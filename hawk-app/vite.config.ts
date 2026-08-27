import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

export default defineConfig({
  root: 'web',
  // 公共目录直指 build/（electron-builder 资源目录），icon 等资产保持单一来源
  publicDir: '../build',
  plugins: [vue()],
  // file:// 打包加载需要相对路径
  base: './',
  server: { port: 5173, strictPort: true },
  build: { outDir: 'dist', emptyOutDir: true },
});

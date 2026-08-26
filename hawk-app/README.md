# hawk-app

桌面应用（Electron 壳 + Vue 3 前端），设计文档见 [../docs/hawk-app.md](../docs/hawk-app.md)。

## 开发

```bash
npm install            # 首次；若 electron 二进制下载慢：export ELECTRON_MIRROR=https://npmmirror.com/mirrors/electron/
dotnet build ../hawk-server   # 后端产物（Electron 开发态直接 dotnet 运行它）
npm run gen:types      # 从 hawk-server 的 OpenAPI schema 生成 TS 类型（web/src/api/schema.d.ts）
npm run dev            # vite + electron 一键起（server 由 electron 拉起）
npm run dev:web        # 只起前端；配合 VITE_HAWK_API / VITE_HAWK_TOKEN 可纯浏览器调试
npm run build          # vue-tsc --noEmit && vite build
```

## 测试

```bash
node tools/ui-check.mjs   # UI 端到端自检：真实启动 electron，CDP 断言 DOM/交互/SSE 并截图
```

## 打包

```bash
scripts/build-server.sh    # 发布当前平台 hawk-server 单文件（可传 RID 交叉编译）
npm run pack               # electron-builder 出安装包（dist/）
```

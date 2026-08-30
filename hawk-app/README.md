# hawk-app

桌面应用（Electron 壳 + Vue 3 前端），设计文档见 [../docs/frontend/hawk-app.md](../docs/frontend/hawk-app.md)。

## 开发

```bash
npm install            # 首次（electron 二进制镜像已配在 .npmrc）
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
npm run pack   # 一条命令：build 前端 + 发布当前平台 hawk-server 单文件 + electron-builder 出包（dist/，Windows 为免安装 portable exe）
```

交叉编译其他平台的 server：node scripts/build-server.mjs <RID>（如 osx-arm64），再单独执行 node scripts/pack.mjs。

国内网络首次 pack 需镜像（下载过的会进 electron-builder 缓存，之后不再需要）：

```bash
export ELECTRON_MIRROR=https://npmmirror.com/mirrors/electron/
export ELECTRON_BUILDER_BINARIES_MIRROR=https://npmmirror.com/mirrors/electron-builder-binaries/
```

压缩级别权衡（ELECTRON_BUILDER_COMPRESSION_LEVEL，默认 5）：9=2分钟/最小体积，5=约1分钟/+11MB，3=约20秒/+27MB。

## CI 发版

`.github/workflows/release.yml`（windows runner，mx=9 压缩）：

- 推 `v*` tag：正式 Release 附 hawk.exe：`git tag v1.0.0 && git push origin v1.0.0`
- main 分支提交信息以 `feat`/`fix` 开头（conventional commits）：滚动覆盖 `nightly` 预发布（PR 合并想触发请用 squash merge）
- Actions 页手动触发：只传 Artifacts，用于验证流程

# hawk-app

桌面应用（Electron 壳 + Vue 3 前端），设计文档见 [../docs/frontend/hawk-app.md](../docs/frontend/hawk-app.md)。

## 开发

```bash
npm install            # 首次（electron 二进制镜像已配在 .npmrc）
cargo build --release --manifest-path ../hawk-daemon/Cargo.toml   # 后端二进制（Electron 开发态直接运行它）
npm run gen:types      # 从 hawk-daemon 的 OpenAPI schema 生成 TS 类型（web/src/api/schema.d.ts）
npm run dev            # vite + electron 一键起（server 由 electron 拉起）
npm run dev:web        # 只起前端；配合 VITE_HAWK_API / VITE_HAWK_TOKEN 可纯浏览器调试
npm run build          # vue-tsc --noEmit && vite build
npm run test:unit      # Vitest 纯函数/决策逻辑单测（web/src/**/*.spec.ts）
```

开发态后端二进制取 `hawk-daemon/target/` 下的构建产物（本机 `release` 优先，其次 `--target` 交叉产物与 `debug`）；
`HAWK_DAEMON_EXE` 环境变量可指向任意二进制覆盖。

## 测试

```bash
npm run test:unit     # Vitest 单测：viewLogic（SSE 决策/排序继承/选择）/importBatch（导入状态机）/layout（齐行布局）
npm run test:mobile   # 移动端网页冒烟：临时库 + hawk-daemon 托管 web/dist + 无 preload 探针窗口断言全链路
npm run test:update   # hawk-update.exe 端到端验证：等进程/覆盖/清理/坏包日志（需先在 hawk-update/ cargo build）
npm run test:resources  # extraResources 平台隔离回归：hawk-update 只进 Windows 产物，mac/linux 只带 hawk-daemon
node tools/ui-check.mjs   # UI 端到端自检：真实启动 electron，CDP 断言 DOM/交互/SSE 并截图
```

## 打包

```bash
npm run pack      # 一条命令：build 前端 + cargo build 当前平台 hawk-daemon 与 hawk-update（Windows 更新辅助）单文件 + electron-builder 出包（dist/，Windows 为免安装 zip）
npm run pack:dir  # 同上但 --dir：只出未打包目录（win-unpacked / hawk.app），跳过 zip 压缩（install 脚本与本地快速验证用）
```

交叉编译其他平台的 server：`node scripts/build-server.mjs <RID>`（win-x64 / osx-arm64 / osx-x64 / linux-x64，别名映射 rust target），再单独执行 `node scripts/pack.mjs`。

国内网络首次 npm install（electron 包二进制）与 pack（electron-builder 下载）需镜像；install.sh/install.ps1 已内置默认，手动构建时自行设置（下载过的会进缓存，之后不再需要）：

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

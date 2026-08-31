<p align="center">
  <img src=".assets/icon.png" width="128" alt="hawk logo">
</p>

<h1 align="center">hawk</h1>

<p align="center">对标 Eagle 的开源图片素材管理工具</p>

<p align="center">
  <a href="https://github.com/soryu-ryouji/hawk/actions/workflows/release.yml"><img src="https://github.com/soryu-ryouji/hawk/actions/workflows/release.yml/badge.svg" alt="release"></a>
  <a href="https://github.com/soryu-ryouji/hawk/releases/latest"><img src="https://img.shields.io/github/v/release/soryu-ryouji/hawk" alt="release version"></a>
  <a href="https://github.com/soryu-ryouji/hawk/releases/tag/nightly"><img src="https://img.shields.io/badge/nightly-滚动预发布-orange" alt="nightly"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-blue" alt="license"></a>
</p>

- 非侵入式资源管理：资源以文件夹的形式进行管理，非侵入式
- 自由开放：开放的 REST API，方便生态接入
- 免费

## 路线图

- 1.0 版本
  - 使用 Rust 重写服务器代码，减小程序体积，提高资源吞吐效率和内存安全性（已完成并替换 C# 版，见 [hawk-server-rs](docs/backend/server-rust.md)）
  - 完成 Windows, MacOS 桌面客户端
  - 完成 Chrome, Firefox, Safari 浏览器插件
  - 完成 web 资源查看器：局域网内通过浏览器访问素材库（设置面板按库配置开关/端口/token，只读查看）；后续拓展 ios, android 查看器
- 2.0 版本
  - 拓展素材资源管理范围，增加 3D 模型， 游戏引擎资源查看功能

## 核心特性

### 非侵入式资源管理

hawk 不会将你的素材"导入"到某个专有仓库中。你的文件始终保存在原来的文件夹里，素材目录中也不会出现任何 hawk 的文件——所有数据都收敛在一个 `.hawk/` 隐藏文件夹中。

- 卸载 hawk 后，你的素材文件纹丝不动
- 原有的文件夹组织习惯完全保留
- 与网盘（Dropbox、iCloud、Syncthing、OneDrive）天然兼容

### 纯文本元数据存储

素材参数（标签、评分、备注等）以独立的纯文本文件存放在 `.hawk/metadata/` 中。网盘同步冲突只影响单个素材，可以用 Git 管理素材库，没有数据锁定。

本机加速：库外缓存目录维护一份 SQLite 派生缓存（`index.db`），启动时一次顺序读即可注水索引（内存索引与元数据都从缓存恢复，秒级就绪），十万级素材无需全量解析 TOML，也无需先全库扫描。缓存只是 TOML 的镜像，可随时删除、重启自动重建，不参与同步。

### 前后端解耦

后端是独立的 Rust 服务，前端只通过 REST API 通信。桌面版用 Electron 壳拉起后端进程；同一套后端未来可直接部署为多人使用的服务器版本。

### 开放 REST API

```text
# 搜索素材
POST http://localhost:27371/api/v1/item/list
{ "keywords": ["logo"], "tags": ["品牌"], "star": 5 }

# 获取缩略图
GET http://localhost:27371/api/v1/item/thumbnail?id=abc123&size=256

# 更新标签
POST http://localhost:27371/api/v1/item/update
{ "id": "abc123", "tags": ["待审核"] }
```

## 本地构建与发布

### 环境准备

- [Node.js](https://nodejs.org/) 与 [Rust 工具链](https://rustup.rs/)（后端为 Rust 实现 `hawk-server-rs/`）
- 克隆仓库后安装依赖（Electron 二进制镜像已配置在 `hawk-app/.npmrc`，国内网络无需额外设置）：

```bash
git clone https://github.com/soryu-ryouji/hawk.git
cd hawk/hawk-app
npm install
cargo build --release --manifest-path ../hawk-server-rs/Cargo.toml
```

### 开发调试

```bash
npm run dev   # vite + electron 一键起（后端由 electron 拉起）
```

更多开发命令见 [hawk-app/README.md](hawk-app/README.md)。

### 发布（打包）

```bash
cd hawk-app
npm run pack
```

一条命令完成：前端构建 → `cargo build --release` 产出当前平台的 hawk-server 单文件（约 9MB，对比 dotnet 自包含的 70MB+）→ electron-builder 打包。产物在 `hawk-app/dist/`：

- **Windows**：`hawk.zip`（绿色软件，解压到任意目录双击 `hawk.exe` 即用；全 64 位，无安装器）
- **macOS**：`dist/mac-arm64/hawk.app`（Intel 机器为 `dist/mac/hawk.app`）——不发 dmg，直接构建 .app 目录；对外分发由 CI zip 成 `hawk-mac-<arch>.zip`
- **Linux**：`hawk.AppImage`

打包默认使用快速压缩（约 1 分钟）。追求最小体积（发正式版）或最快速度（冒烟验证）可用环境变量调整：

```bash
ELECTRON_BUILDER_COMPRESSION_LEVEL=9 npm run pack   # 最小体积，约 2 分钟
ELECTRON_BUILDER_COMPRESSION_LEVEL=3 npm run pack   # 最快，约 20 秒（体积 +27MB）
```

交叉编译其他平台的后端：`node scripts/build-server.mjs <RID>`（如 `osx-arm64`，内部映射 rust target 并自动 `rustup target add`），再单独执行 `node scripts/pack.mjs`。

把应用与浏览器插件构建并拷贝到指定目录（本地安装用）：

```powershell
./tools/build.ps1 --platform app,ext-chrome,ext-firefox --path D:/Tools/hawk
```

- `--platform` 可选 `app` / `ext-chrome` / `ext-firefox`，逗号分隔，默认全部；浏览器插件见 [hawk-browser-extension](hawk-browser-extension/README.md)（Safari 需 macOS 另行转换，不参与此脚本）
- `--path` 产物输出目录，默认 `<仓库>/out/`；应用直接输出到该目录根下——Windows 自动解压 `hawk.zip`（`hawk.exe` 就地可运行）、macOS 为 `hawk.app` 目录、Linux 为 `hawk.AppImage`；插件输出为 `hawk-extension-chrome|firefox/` 目录（浏览器「加载已解压扩展程序」直接用）
- 有请求的内容未构建成功（失败或跳过）时退出码为 1

### CI 发版

发版一般在 CI 上完成（见 [.github/workflows/release.yml](.github/workflows/release.yml)），无需本地打包：

- 推 `v*` tag（如 `git tag v1.0.0 && git push origin v1.0.0`）：自动构建并创建正式 Release
- main 分支上 `feat`/`fix` 开头的提交：自动滚动更新 nightly 预发布
- 每次发版产物：Windows `hawk.zip` + macOS `hawk-mac-arm64.zip` / `hawk-mac-x64.zip`（.app 目录打 zip；同一 arm64 runner 交叉打包，x64 无需额外机器）

## 文档

**总体**

- [架构设计](docs/architecture.md)：进程模型、桌面/服务器部署形态、仓库结构
- [技术栈](docs/tech-stack.md)：语言与框架选型

**前端**（[hawk-app](hawk-app/README.md)）

- [hawk-app 设计](docs/frontend/hawk-app.md)：Electron 壳 + Vue 前端的界面与接入设计

**后端（hawk-server-rs/）**

- [hawk-server-rs（Rust 实现）](docs/backend/server-rust.md)：Rust 版实现细节、调试与压测方法
- [hawk-server 代码导读](docs/backend/server-code-structure.md)：逐文件职责与关键流程串联
- [REST API V1](docs/backend/server-rest-api-v1.md)：接口定义
- [Category 虚拟分类维度](docs/backend/category.md)：三维组织模型、注册表与批量迁移
- [颜色检索](docs/backend/color-search.md)：调色板提炼与 ΔE 颜色匹配
- [存储设计](docs/backend/storage.md)：`.hawk/` 目录结构、同步边界、索引与缓存

## 许可证

[AGPL-3.0](LICENSE)

# 架构设计

## 总体架构

hawk 前后端完全解耦，通过 HTTP API 通信。桌面版中 Electron 只是一个壳：启动时拉起后端进程，退出时一起回收。后端是同一份代码，未来可直接部署为服务器版本。

```text
┌──────────────────────────────────────────────────────┐
│                    用户素材目录                        │
│   design.jpg  photo.png  ...（无任何 hawk 文件）       │
│   .hawk/                                             │
│     ├── config.toml   项目配置（参与同步）              │
│     ├── categories.toml / tags.toml / view.toml /    │
│     │   global_filter.toml   注册表与偏好（参与同步）   │
│     ├── metadata/     素材参数（配置文件模式，参与同步）  │
│     ├── metadata.db   素材参数（数据库模式，本地专用）   │
│     ├── storage_mode  存储方案标记（参与同步）          │
│     └── trash/        回收站（本地专用）                │
│   缩略图/调色板/元数据缓存 → 库外系统缓存目录（本地专用）  │
└──────────────────────┬───────────────────────────────┘
                       │ 文件系统监听 / 读写
                       ▼
┌──────────────────────────────────────────────────────┐
│                 hawk-daemon (Rust)                          │
│  ┌───────────┐  ┌───────────┐  ┌───────────────────┐  │
│  │ Watcher   │  │ Hash      │  │ Thumbnail         │  │
│  │ (notify)  │  │ (blake3)  │  │ (image+libwebp)   │  │
│  └───────────┘  └───────────┘  └───────────────────┘  │
│  ┌────────────────────────┐                             │
│  │ In-Memory Index/Search │                             │
│  │ (启动时扫描比对)        │                             │
│  └────────────────────────┘                             │
│       REST API (axum, 静态 OpenAPI schema)               │
└──────────────────────┬───────────────────────────────┘
                       │ HTTP
          ┌────────────┼────────────┐
          ▼            ▼            ▼
    ┌──────────┐ ┌──────────┐ ┌──────────┐
    │ Web 前端  │ │ 生态接入  │ │ CLI 工具 │
    │  (Vue 3) │ │ 各类插件  │ │          │
    └──────────┘ └──────────┘ └──────────┘
```

详见 [技术栈](tech-stack.md) 与 [存储设计](backend/storage.md)。

## 核心原则

**1. 前端只认识 HTTP API**

Web 前端不依赖 Electron IPC，只通过 REST API 通信。

**2. 后端不依赖 Electron**

后端是独立的 Rust 二进制，不依赖任何桌面端代码。

**3. API 契约先行**

REST API 由代码生成 OpenAPI schema（utoipa：`#[utoipa::path]` 标注端点、DTO derive `ToSchema`，路由与文档同一来源），固化于 `hawk-daemon/openapi.json`（改 API 后 `cargo run -- --dump-openapi > openapi.json` 重新固化，契约测试校验同步），TypeScript 类型从 schema 生成（`npm run gen:types`），前后端不允许手写对接口。

**4. 编辑计算归客户端，server 归存储与管理**

图片编辑（旋转、裁切等）的解码/变换/重编码在客户端完成（Web 标准 canvas 能力），server 只提供存储层能力（`item/replace` 内容替换：校验、哈希、写盘、id 漂移闭环）。server 部署为远程服务时不承接用户图片的编辑计算。

## 部署形态

### 桌面版（sidecar 模式）

Electron 壳与后端二进制一起打包发行（electron-builder 的 `extraResources`）：

```text
Electron 启动
  → 预选空闲环回端口 + 生成随机 token
  → spawn hawk-daemon 子进程
      - 参数 --library <path> --port <预选端口>（远程查看模式不传 --library，daemon 无库形态运行），token 经环境变量传入
      - **先监听端口，初始索引后台构建**（先监听、后索引的启动模型）
  → 轮询 GET /api/v1/app/startup（200ms 间隔）：
      starting → 进度帧驱动启动进度页；ready → 加载主界面；error → 弹错误框
  → 初始索引完成前 /api/* 返回 503 NOT_READY（app/startup 除外），/health 503
Electron 退出
  → 回收 hawk-daemon 子进程（防止孤儿进程残留）
```

**本地 API 安全**：localhost 端口任何本机进程都能访问，因此所有请求必须携带启动时生成的随机 token。token 只存在于进程环境变量中，不落盘。防护对象是浏览器里的恶意网页（CSRF 直写素材库）——本机同权限进程可直接读写素材目录，等价绕过，不在防护范围。生态客户端（浏览器插件等）通过本地 API 端口接入：**端口由用户配置决定**（`~/.config/hawk/config.toml` 的 `apiPort`，默认 27371），daemon 严格绑定、绑不上就报错退出（不静默漂移），app 在启动屏询问用户换端口（自动选择限 27371–27400 段，手输允许任意）。为免配置，提供免鉴权的 token 发现端点 `GET /api/v1/app/token`：响应不带 CORS 头（跨源网页 JS 读不到，持 host_permissions 的扩展可读）且 Host 限定环回地址（防 DNS rebinding 同源绕过）；浏览器插件按同一端口段顺序探活该端点即零配置接入（手填端口仅作覆盖，见 hawk-browser-extension / lib/local-discovery.ts），等价 Eagle 的无鉴权体验但不牺牲防护。

### 服务器版（未来）

同一个 hawk-daemon 直接部署在服务器上，为多个用户服务。与桌面版的差异：

| 差异点   | 桌面版             | 服务器版                      |
| -------- | ------------------ | ----------------------------- |
| 数据来源 | 监听本地文件系统   | 用户上传 / 同步客户端推送     |
| 索引存储 | 内存索引           | 集中式数据库（如 PostgreSQL） |
| 认证     | 随机 token（单机） | 真实的用户认证体系            |

这三处差异在架构上已预留位置：core 的索引读写收在窄接口后面、API 预留认证头位置。但**当前只实现桌面版**，不提前实现服务器版的任何功能。

### 远程访问（服务端与 daemon remote 模块已实现）

远程访问不走服务器版路线：每台设备只有 hawk-app 与 hawk-daemon 两个进程，广域网连接能力（信令、心跳、iroh 隧道、会话代理；打洞/中继/路径迁移由 iroh 承担）全部收在 daemon 的 **`remote/` 模块**——可选子系统，应用级 `remote.toml` 启用才激活，supervisor reconcile 管理（同 LAN web 先例）；隧道内请求在进程内转发并标记 viewer 级权限或受限远程写权限（写会话需带写权限的访问密钥 + 目标设备开关 + daemon 本地 key 执法），无需受托 token 机制。hawk-app 主进程只做连接描述符与断线回切（订阅 daemon SSE 的 remote 事件）。远程查看时数据面为端到端 QUIC 直连，素材数据始终由素材库所在机的 daemon 产出，复用现有 API 与只读/受限写鉴权。远程访问服务端 **hawk-server**（本仓库 `hawk-server/`，独立 crate）为多租户自托管方案：无账号/会员体系，准入即程序生成的租户 service-key（`tenant` CLI 管理，服务端只存 HMAC 哈希，凭证验证面带 IP/租户风控）；查看者两级验证（service-key 换门令牌 → hawk-app key 换 bt）。与客户端零代码共享，协议契约是唯一共享物。数据面直连优先（服务器只做撮合、不承素材带宽），中继为服务端可选组件（已实现）。设计见 [远程访问设计](backend/remote-access.md)，接口契约见 [remote-protocol](backend/remote-protocol.md)，服务端使用见 [hawk-server 使用指南](backend/hawk-server.md)。

### 移动端（iOS / Android 原生，M1 只读浏览已实现）

移动端不走 WebView 壳，也不做服务器版：客户端只吃 HTTP 契约，局域网直连 daemon，广域网经 hawk-server 查看面（服务端隧道代理 `/api/v1/*`）。选原生的原因是系统级集成（iOS 的 App Intents / Spotlight / Foundation Models，Android 的 App Actions / 系统搜索）只有原生可用，WebView 拿不到。客户端不链接 AGPL 代码，许可证与上架不受约束。iOS 与 Android 各自直写原生 UI，暂不抽象跨平台共享层。

- iOS：SwiftUI，M1 只读浏览已实现，设计见 [iOS 客户端设计](frontend/ios-app.md)
- Android：Kotlin + Jetpack Compose，M1 只读浏览已实现，设计见 [Android 客户端设计](frontend/android-app.md)

## 仓库结构

```text
hawk/
├── hawk-daemon/             ← Rust 后端（桌面版与服务器版共用）
├── hawk-update/             ← Rust Windows 更新辅助程序（桌面端更新安装接力，仅 Windows 产物携带）
├── hawk-app/                ← 桌面应用（Electron 壳 + Vue 前端，见 docs/frontend/hawk-app.md）
├── hawk-browser-extension/  ← 浏览器插件（Chrome/Firefox，WXT）
├── hawk-server/             ← Rust 远程访问服务端（控制面/中继/查看面，见 docs/backend/remote-access.md）
├── hawk-ios/                ← iOS 原生客户端（M1 只读浏览，见 docs/frontend/ios-app.md）
├── hawk-android/            ← Android 原生客户端（M1 只读浏览，见 docs/frontend/android-app.md）
├── tools/                   ← 仓库级脚本（构建/安装/冒烟/性能压测/文档链接检查）
└── docs/                    ← 设计文档
```

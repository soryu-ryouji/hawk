# 架构设计

## 总体架构

hawk 是纯局域网素材管理工具，前后端完全解耦，通过 HTTP API 通信。桌面版中 Electron 只是壳：启动时拉起 Go 后端进程，退出时一起回收。后端是同一份代码，桌面、手机、浏览器插件都直接访问它。

```text
┌──────────────────────────────────────────────────────┐
│                    用户素材目录                        │
│   design.jpg  photo.png  ...（无任何 hawk 文件）       │
│   .hawk/                                             │
│     ├── config.toml   项目配置（参与同步）              │
│     ├── categories.toml / tags.toml / view.toml /    │
│     │   global_filter.toml / locks.toml   注册表       │
│     ├── metadata.db   素材元数据（SQLite，本地专用）     │
│     └── trash/        回收站（本地专用）                │
│   缩略图 → 库外系统缓存目录（本地专用）                  │
└──────────────────────┬───────────────────────────────┘
                       │ 文件系统监听 / 读写
                       ▼
┌──────────────────────────────────────────────────────┐
│                 hawk-daemon (Go)                     │
│  ┌───────────┐  ┌───────────┐  ┌───────────────────┐│
│  │ Watcher   │  │ Hash      │  │ Thumbnail          ││
│  │ (fsnotify)│  │ (blake3)  │  │ (image + webp)     ││
│  └───────────┘  └───────────┘  └───────────────────┘│
│  ┌────────────────────────┐                         │
│  │ In-Memory Index/Search │                         │
│  │ (启动时 SQLite 注水)    │                         │
│  └────────────────────────┘                         │
│       REST API + SSE (net/http)                     │
└──────────────────────┬───────────────────────────────┘
                       │ HTTP（局域网明文 + AccessKey）
          ┌────────────┼────────────┬────────────┐
          ▼            ▼            ▼            ▼
    ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐
    │ Web 前端 │ │ iOS      │ │ Android  │ │ 浏览器插件│
    │ (Vue 3)  │ │ SwiftUI  │ │ Compose  │ │           │
    └──────────┘ └──────────┘ └──────────┘ └──────────┘
```

详见 [技术栈](tech-stack.md) 与 [存储设计](backend/storage.md)。

## 核心原则

**1. 前端只认识 HTTP API**

Web 前端不依赖 Electron IPC，只通过 REST API 通信。

**2. 后端不依赖 Electron**

后端是独立的 Go 二进制，不依赖任何桌面端代码。

**3. API 契约先行**

REST API 由 `hawk-daemon/openapi.json` 固化契约；Go handler 与 DTO 按它实现，契约测试校验嵌入副本同步。TypeScript 类型从 schema 生成（`npm run gen:types`），前后端不允许手写对接口。

**4. 编辑计算归客户端，server 归存储与管理**

图片编辑（旋转、裁切等）的解码/变换/重编码在客户端完成（Web 标准 canvas 能力），server 只提供存储层能力（`item/replace` 内容替换：校验、哈希、写盘、id 漂移闭环）。

**5. 局域网明文，不做传输加密**

产品边界是同一可信局域网。传输层不做 TLS/QUIC 加密，访问控制由目标 daemon 的 AccessKey + 能力表强制执行。需要跨网段时由用户自行用 Tailscale 等工具组网。

## 部署形态

### 桌面版（sidecar 模式）

Electron 壳与后端二进制一起打包发行（electron-builder 的 `extraResources`）：

```text
Electron 启动
  → 预选空闲环回端口 + 生成随机 token
  → spawn hawk-daemon 子进程
      - `local <素材库路径> --port <端口> --web-dist <dist> --identity-dir <dir>`
      - token 经环境变量 HAWK_ADMIN_TOKEN 传入
      - 先监听端口，初始索引后台构建
  → 轮询 GET /api/v1/app/startup（200ms 间隔）：
      starting → 进度帧驱动启动进度页；ready → 加载主界面；error → 弹错误框
  → 初始索引完成前 /api/* 返回 503 NOT_READY（app/startup 除外）
Electron 退出
  → 回收 hawk-daemon 子进程
```

**本地 API 安全**：localhost 端口任何本机进程都能访问，因此所有请求必须携带启动时生成的随机 token。token 只存在于进程环境变量中，不落盘。防护对象是浏览器里的恶意网页（CSRF 直写素材库）——本机同权限进程可直接读写素材目录，等价绕过，不在防护范围。生态客户端（浏览器插件等）通过本地 API 端口接入：端口由用户配置决定（`~/.config/hawk/config.toml` 的 `apiPort`，默认 27371），daemon 严格绑定、绑不上就报错退出，app 在启动屏询问用户换端口。为免配置，提供免鉴权的 token 发现端点 `GET /api/v1/app/admin-token`：响应不带 CORS 头且 Host 限定环回地址；插件按端口段探活即零配置接入。

### 局域网访问

daemon 提供两个独立监听器：

- **Web 端口**（默认 27372）：托管 Web 前端，浏览器用 AccessKey 换 HttpOnly Session Cookie；写请求校验 Origin 与 `X-Hawk-Client`。
- **Peer 端口**（默认 27375）：给 hawk-app / iOS / Android 直连，Bearer AccessKey，不托管网页。

两个端口都只监听局域网，不做 TLS。访问密钥由目标端签发、按库隔离；每把密钥独立 viewer/editor 角色，删除或降权在下一请求生效。

### 局域网发现与配对

- UDP 27374 广播 probe/announce，公告只含设备名、库名、端口与写能力提示，不含任何 token。
- 桌面端之间通过 `POST /api/v1/app/lan/pair-requests` 发起配对，目标端用户确认后签发专用 AccessKey；一次审批可双向保存回连 profile。
- 手机端通过二维码或手动输入「地址 + AccessKey」直连，二维码载荷只含 `access_key`、`lan_endpoints` 与库标识。

### 移动端（iOS / Android 原生，M1 只读浏览已实现）

移动端不走 WebView 壳，也不做服务器版：客户端只吃 HTTP 契约，局域网直连 daemon。选原生的原因是系统级集成（iOS 的 App Intents / Spotlight / Foundation Models，Android 的 App Actions / 系统搜索）只有原生可用。客户端不链接 AGPL 代码，许可证与上架不受约束。

- iOS：SwiftUI，设计见 [iOS 客户端设计](frontend/ios-app.md)
- Android：Kotlin + Jetpack Compose，设计见 [Android 客户端设计](frontend/android-app.md)

## 仓库结构

```text
hawk/
├── hawk-daemon/             ← Go 素材库 daemon
├── hawk-update/             ← Rust Windows 更新辅助程序（桌面端更新安装接力，仅 Windows 产物携带）
├── hawk-app/                ← 桌面应用（Electron 壳 + Vue 前端）
├── hawk-browser-extension/  ← 浏览器插件（Chrome/Firefox，WXT）
├── hawk-ios/                ← iOS 原生客户端
├── hawk-android/            ← Android 原生客户端
├── tools/                   ← 仓库级脚本（构建/安装/冒烟/压测/文档链接检查）
└── docs/                    ← 设计文档
```

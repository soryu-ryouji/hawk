# 架构设计

## 总体架构

hawk 前后端完全解耦，通过 HTTP API 通信。桌面版中 Electron 只是一个壳：启动时拉起后端进程，退出时一起回收。后端是同一份代码，未来可直接部署为服务器版本。

```text
┌──────────────────────────────────────────────────────┐
│                    用户素材目录                        │
│   design.jpg  photo.png  ...（无任何 hawk 文件）       │
│   .hawk/                                             │
│     ├── config.toml   项目配置（参与同步）              │
│     ├── metadata/     素材参数（参与同步）              │
│     └── trash/        回收站（本地专用）                │
│   缩略图/调色板缓存 → 库外系统缓存目录（本地专用）       │
└──────────────────────┬───────────────────────────────┘
                       │ 文件系统监听 / 读写
                       ▼
┌──────────────────────────────────────────────────────┐
│                 hawk-daemon (Rust)                          │
│  ┌───────────┐  ┌───────────┐  ┌───────────────────┐  │
│  │ Watcher   │  │ Hash      │  │ Thumbnail         │  │
│  │(notify)   │  │(blake3)    │  │ (image+libwebp)   │  │
│  │ Watcher)  │  │           │  │                   │  │
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

REST API 由 OpenAPI schema 定义（后端从代码生成），TypeScript 类型从 schema 生成，前后端不允许手写对接口。

**4. 编辑计算归客户端，server 归存储与管理**

图片编辑（旋转、裁切等）的解码/变换/重编码在客户端完成（Web 标准 canvas 能力），server 只提供存储层能力（`item/replace` 内容替换：校验、哈希、写盘、id 漂移闭环）。server 部署为远程服务时不承接用户图片的编辑计算。

## 部署形态

### 桌面版（sidecar 模式）

Electron 壳与后端二进制一起打包发行（electron-builder 的 `extraResources`）：

```text
Electron 启动
  → 预选空闲环回端口 + 生成随机 token
  → spawn hawk-daemon 子进程
      - 参数 --library <path> --port <预选端口>，token 经环境变量传入
      - **先监听端口，初始索引后台构建**（先监听、后索引的启动模型）
  → 轮询 GET /api/v1/app/startup（200ms 间隔）：
      starting → 进度帧驱动启动进度页；ready → 加载主界面；error → 弹错误框
  → 初始索引完成前 /api/* 返回 503 NOT_READY（app/startup 除外），/health 503
Electron 退出
  → 回收 hawk-daemon 子进程（防止孤儿进程残留）
```

**本地 API 安全**：localhost 端口任何本机进程都能访问，因此所有请求必须携带启动时生成的随机 token。token 只存在于进程环境变量中，不落盘。防护对象是浏览器里的恶意网页（CSRF 直写素材库）——本机同权限进程可直接读写素材目录，等价绕过，不在防护范围。生态客户端（浏览器插件等）通过默认端口 27371 连接。为免配置，提供免鉴权的 token 发现端点 `GET /api/v1/app/token`：响应不带 CORS 头（跨源网页 JS 读不到，持 host_permissions 的扩展可读）且 Host 限定环回地址（防 DNS rebinding 同源绕过），插件零配置即可接入，等价 Eagle 的无鉴权体验但不牺牲防护。

### 服务器版（未来）

同一个 hawk-daemon 直接部署在服务器上，为多个用户服务。与桌面版的差异：

| 差异点   | 桌面版             | 服务器版                      |
| -------- | ------------------ | ----------------------------- |
| 数据来源 | 监听本地文件系统   | 用户上传 / 同步客户端推送     |
| 索引存储 | 内存索引           | 集中式数据库（如 PostgreSQL） |
| 认证     | 随机 token（单机） | 真实的用户认证体系            |

这三处差异在架构上已预留位置：core 的索引读写收在窄接口后面、API 预留认证头位置。但**当前只实现桌面版**，不提前实现服务器版的任何功能。

### 远程访问（规划中）

远程访问不走服务器版路线，由三个进程协作：hawk-daemon 本体保持桌面版定位不变，唯一的调用点是鉴权中间件多认一种 env 注入的受托只读 token（无 remote 语义、无需门控）；广域网连接能力全部收在独立进程 **hawk-remote**（仓库内独立项目，随 AGPL 开源），负责信令、心跳、UPnP、QUIC 隧道与本地代理；hawk-app 主进程是接线枢纽（拉起/传参/回收、连接描述符）。远程查看时数据面为端到端 QUIC 隧道，素材数据始终由素材库所在机的 daemon 产出，复用现有 API 与只读鉴权。云端服务（hawk-server：hub + relay，闭源独立仓库）与 hawk 仓库只共享协议契约、零代码共享。产物裁剪在打包层：社区构建不编译、不携带 hawk-remote，前端不定义 HAWK_REMOTE。设计见 [远程访问设计](backend/remote-access.md)，接口契约见 [remote-protocol](backend/remote-protocol.md)。

## 仓库结构

```text
hawk/
├── hawk-daemon/  ← Rust 后端（桌面版与服务器版共用）
├── hawk-update/  ← Rust Windows 更新辅助程序（桌面端更新安装接力，仅 Windows 产物携带）
├── hawk-app/        ← 桌面应用（Electron 壳 + Vue 前端，见 docs/frontend/hawk-app.md）
├── hawk-remote/  ← Rust 远程访问客户端（可选进程，见 docs/backend/remote-access.md，规划中）
└── docs/            ← 设计文档
```

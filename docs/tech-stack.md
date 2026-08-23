# 技术栈

## 总览

```text
Rust (axum + rusqlite + notify + blake3 + image)  ← 后端，桌面/服务器共用
React + TypeScript (Vite)                          ← 前端，只走 HTTP
Electron                                           ← 桌面壳，sidecar 拉起后端
OpenAPI (utoipa 生成)                              ← 前后端契约
```

## 后端：Rust

| 职责 | 选型 | 实现要点 |
|---|---|---|
| HTTP 框架 | axum | tokio 运行时 |
| SQLite | rusqlite | `bundled` 特性内置编译 SQLite，开启 FTS5 |
| 文件监听 | notify | 跨平台 |
| 哈希 | blake3 | 官方 crate，SIMD |
| 图像解码 | image | 覆盖 JPEG/PNG/GIF/WebP/TIFF/BMP/EXR 等 |
| 图像缩放 | fast_image_resize | 生成缩略图，SIMD 加速 |
| 配置 | serde + toml | 反序列化即校验 |
| OpenAPI | utoipa | 从 handler 代码生成 schema |
| 日志 | tracing | |

### 图像格式扩展路线

MVP 用 `image` crate 覆盖常见格式。图像解码收在一个接口后面，后续按需扩展：

- RAW：`rawler`
- HEIC：libheif 绑定
- 更全的格式：libvips

### 并发模型

索引流水线（监听 → 哈希 → 缩略图 → 入库）用标准库线程 + channel；tokio 只服务 HTTP 层。首次大批量索引时 worker 池需要做好背压，避免把磁盘 IO 打满。

## 前端：React + TypeScript

| 职责 | 选型 | 实现要点 |
|---|---|---|
| 框架 | React | |
| 构建 | Vite | |
| 数据获取 | TanStack Query | 所有数据经 REST API 获取 |
| 虚拟滚动 | TanStack Virtual | 素材网格布局 |

## 桌面壳：Electron

- electron-builder 打包，`extraResources` 携带各平台后端二进制
- 主进程只负责：创建窗口、拉起/回收后端进程、注入 token

## 契约：OpenAPI

- Rust 侧用 utoipa 从 handler 代码生成 OpenAPI schema
- TypeScript 类型从 schema 生成（如 openapi-typescript）
- CI 校验 schema 与代码一致，防止前后端漂移

# 技术栈

## 总览

```text
后端：Rust
前端：Vue 3 + TypeScript (Vite)，只走 HTTP
桌面壳：Electron，sidecar 拉起后端
契约：OpenAPI
```

## 后端

Rust，见 [hawk-daemon](backend/server-rust.md)（`hawk-daemon/`，axum、notify、blake3、image + fast_image_resize、rusqlite）。
已实现全部 API/存储/事件契约，为 app 唯一后端。

## 远程访问客户端：hawk-remote/（规划中）

独立 Rust 进程，承载全部广域网连接能力（信令/心跳/WSS/UPnP/QUIC 隧道/本地代理），见 [远程访问设计](backend/remote-access.md)。技术栈：tokio、axum、quinn、rcgen、igd、tokio-tungstenite。

## 前端与桌面壳：hawk-app/

Electron 壳 + Vue 3 前端（Composition API + `<script setup>` + Pinia），设计见 [hawk-app 设计](frontend/hawk-app.md)。约束：

- 前端只通过 REST API 与后端通信，不依赖 Electron IPC
- Electron 主进程只负责：创建窗口、拉起/回收后端进程、注入 token
- electron-builder 打包，`extraResources` 携带各平台后端二进制

## 契约：OpenAPI

- OpenAPI schema 固化于 `hawk-daemon/openapi.json`，由后端静态服务（`/openapi/v1.json`）
- TypeScript 类型从 schema 生成（如 openapi-typescript）
- CI 校验 schema 与代码一致，防止前后端漂移

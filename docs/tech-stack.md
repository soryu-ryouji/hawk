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

- OpenAPI schema 由后端代码生成（utoipa：`#[utoipa::path]` + `ToSchema` derive，路由即文档），固化于 `hawk-daemon/openapi.json`（`cargo run -- --dump-openapi` 重新生成）
- 固化文件与代码的同步由契约测试保证（`cargo test`：`api/contract_tests.rs` 的 `openapi_json_in_sync`）；同时校验全部端点的真实响应符合 schema、SSE 事件名与 `SseEvents` 双向一致
- TypeScript 类型从 schema 生成（openapi-typescript，`npm run gen:types`）；CI 校验生成产物与仓库同步

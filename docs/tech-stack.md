# 技术栈

## 总览

```text
后端：Rust
前端：Vue 3 + TypeScript (Vite)，只走 HTTP
桌面壳：Electron，sidecar 拉起后端
移动端：Swift + SwiftUI（iOS 26+，原生，复用同一套 HTTP 契约）
契约：OpenAPI
```

## 后端

Rust，见 [hawk-daemon](backend/server-rust.md)（`hawk-daemon/`，axum、notify、blake3、image + fast_image_resize、rusqlite）。
已实现全部 API/存储/事件契约，为 app 唯一后端。

## 远程访问服务端：hawk-server/

独立 Rust crate（与 daemon 零代码共享），多租户方案：租户管理（tenant CLI）/设备注册/心跳/WSS 控制通道/会话撮合/key 映射/两级登录与风控（P1 已实现，中继二期），见 [远程访问设计](backend/remote-access.md)、[协议契约](backend/remote-protocol.md) 与 [hawk-server 使用指南](backend/hawk-server.md)。技术栈：tokio、axum（含 ws）、serde_json（注册表 JSON 持久化）；无数据库、无账号体系，准入即程序生成的租户 service-key（服务端只存 HMAC 哈希），广域网写用逐把带权限的 hawk-app key。

## 远程访问客户端：hawk-daemon 的 remote/ 模块（信令进行中）

广域网连接能力（信令/心跳/WSS/UPnP/QUIC 隧道/会话代理）收编为 daemon 内模块，应用级远程配置（`remote.toml`）启用才激活（supervisor reconcile，同 LAN web 先例），见 [远程访问设计](backend/remote-access.md)。新增依赖：tokio-tungstenite、reqwest（rustls）、rcgen、sha2；数据面阶段增加 quinn、igd。

## 前端与桌面壳：hawk-app/

Electron 壳 + Vue 3 前端（Composition API + `<script setup>` + Pinia），设计见 [hawk-app 设计](frontend/hawk-app.md)。约束：

- 前端只通过 REST API 与后端通信，不依赖 Electron IPC
- Electron 主进程只负责：创建窗口、拉起/回收后端进程、注入 token
- electron-builder 打包，`extraResources` 携带各平台后端二进制

## 移动端：hawk-ios/（规划中）

原生 iOS / iPadOS 客户端，设计见 [iOS 客户端设计](frontend/ios-app.md)。约束：

- Swift 6 + SwiftUI，部署目标 iOS 26.0，iPhone/iPad 单 target；不使用 WebView 壳、Kotlin Multiplatform、Flutter
- 只走 HTTP 契约：局域网直连 daemon，广域网经 hawk-server 查看面（`gate`/`login` 换 `bt` 后由服务端隧道代理 `/api/v1/*`），不链接 AGPL 代码
- SwiftPM 依赖：GRDB（本地索引，M2）；iroh-ffi（设备内直连，M4 视需要）
- AI 能力：App Intents（Spotlight 语义索引，全机型）、Foundation Models（本地推理，iOS 26+ 且需支持 Apple Intelligence）、Vision（OCR/分类，全机型）

## 契约：OpenAPI

- OpenAPI schema 由后端代码生成（utoipa：`#[utoipa::path]` + `ToSchema` derive，路由即文档），固化于 `hawk-daemon/openapi.json`（`cargo run -- --dump-openapi` 重新生成）
- 固化文件与代码的同步由契约测试保证（`cargo test`：`api/contract_tests.rs` 的 `openapi_json_in_sync`）；同时校验全部端点的真实响应符合 schema、SSE 事件名与 `SseEvents` 双向一致
- TypeScript 类型从 schema 生成（openapi-typescript，`npm run gen:types`）；CI 校验生成产物与仓库同步

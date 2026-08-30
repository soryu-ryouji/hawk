# 技术栈

## 总览

```text
后端：Rust（app 默认后端）；C# 过渡版保留参考
前端：Vue 3 + TypeScript (Vite)，只走 HTTP
桌面壳：Electron，sidecar 拉起后端
契约：OpenAPI
```

## 后端

分两阶段：

1. **过渡实现**：C#，见 [hawk-server（C# 过渡实现）](backend/server-csharp.md)。API 设计已验证完成，**代码保留至 Rust 版稳定**
2. **目标实现**：Rust，见 [hawk-server-rs](backend/server-rust.md)（`hawk-server-rs/`，axum、notify、blake3、image + fast_image_resize、rusqlite）。已实现全部 API/存储/事件契约，与 C# 版并行调试中（冒烟测试双实现同跑：`tools/smoke.sh [rust]`）

## 前端与桌面壳：hawk-app/

Electron 壳 + Vue 3 前端（Composition API + `<script setup>` + Pinia），设计见 [hawk-app 设计](frontend/hawk-app.md)。约束：

- 前端只通过 REST API 与后端通信，不依赖 Electron IPC
- Electron 主进程只负责：创建窗口、拉起/回收后端进程、注入 token
- electron-builder 打包，`extraResources` 携带各平台后端二进制

## 契约：OpenAPI

- 后端从代码生成 OpenAPI schema（具体工具见对应阶段的后端文档）
- TypeScript 类型从 schema 生成（如 openapi-typescript）
- CI 校验 schema 与代码一致，防止前后端漂移

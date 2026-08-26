# 技术栈

## 总览

```text
后端：C#（过渡）→ Rust（目标）
前端：Vue 3 + TypeScript (Vite)，只走 HTTP
桌面壳：Electron，sidecar 拉起后端
契约：OpenAPI
```

## 后端

分两阶段：

1. **过渡实现（当前）**：C#，见 [hawk-server（C# 过渡实现）](server-csharp.md)
2. **目标实现**：Rust。待 API 设计验证完成、C# 版本跑通后整体替换。目标栈：axum、notify、blake3、image + fast_image_resize，索引同样在内存中维护

## 前端与桌面壳：hawk-app/

Electron 壳 + Vue 3 前端（Composition API + `<script setup>` + Pinia），设计见 [hawk-app 设计](hawk-app.md)。约束：

- 前端只通过 REST API 与后端通信，不依赖 Electron IPC
- Electron 主进程只负责：创建窗口、拉起/回收后端进程、注入 token
- electron-builder 打包，`extraResources` 携带各平台后端二进制

## 契约：OpenAPI

- 后端从代码生成 OpenAPI schema（具体工具见对应阶段的后端文档）
- TypeScript 类型从 schema 生成（如 openapi-typescript）
- CI 校验 schema 与代码一致，防止前后端漂移

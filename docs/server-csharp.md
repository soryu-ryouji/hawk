# hawk-server（C# 过渡实现）

第一版后端使用 C# 实现，目的是验证 API 设计、跑通完整后端流程。验证完成后将整体替换为 Rust 实现。

替换对前端透明：前端只依赖 OpenAPI 契约，不感知后端语言。因此 C# 阶段就要保证 OpenAPI schema 的准确性和完整性，schema 即契约。

## 技术选型

| 职责 | 选型 | 实现要点 |
|---|---|---|
| HTTP 框架 | ASP.NET Core Minimal API | |
| SQLite | Microsoft.Data.Sqlite | 内置 e_sqlite3，开启 FTS5 |
| 文件监听 | FileSystemWatcher | 内置 |
| 哈希 | Blake3.NET | |
| 图像处理 | ImageSharp | 解码 JPEG/PNG/GIF/WebP/TIFF/BMP，缩放生成缩略图 |
| 配置 | Tomlyn | TOML 解析 |
| OpenAPI | Microsoft.AspNetCore.OpenApi | 从代码生成 schema |
| 日志 | ILogger（Serilog Provider） | |

## 发布

`dotnet publish` 自包含单文件，按平台（win-x64 / osx-arm64 / linux-x64）产出 hawk-server 二进制，随 Electron 一起打包。NativeAOT 作为后续优化项，C# 阶段不启用。

## 图像格式扩展路线

MVP 用 ImageSharp 覆盖常见格式。图像解码收在一个接口后面，RAW、HEIC 等格式后续再定实现方案。

## 并发模型

索引流水线（监听 → 哈希 → 缩略图 → 入库）用 `System.Threading.Channels` 串联，有界 channel 提供背压；HTTP 层由 ASP.NET Core 处理。

## 替换为 Rust 时的注意点

- 目标栈：axum、rusqlite（FTS5）、notify、blake3、image + fast_image_resize
- 行为对齐以 OpenAPI schema 为准，不逐行翻译 C# 代码
- `.hawk/` 存储格式（metadata TOML、hawk.db 结构、缩略图命名）属于持久化契约，Rust 版必须兼容

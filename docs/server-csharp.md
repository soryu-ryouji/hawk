# hawk-server（C# 过渡实现）

> 逐文件的代码职责与流程串联见 [代码导读](server-code-structure.md)。

第一版后端使用 C# 实现，目的是验证 API 设计、跑通完整后端流程。验证完成后将整体替换为 Rust 实现。

替换对前端透明：前端只依赖 OpenAPI 契约，不感知后端语言。因此 C# 阶段就要保证 OpenAPI schema 的准确性和完整性，schema 即契约。

## 技术选型

| 职责      | 选型                         | 实现要点                                               |
| --------- | ---------------------------- | ------------------------------------------------------ |
| HTTP 框架 | ASP.NET Core Minimal API     |                                                        |
| 文件监听  | FileSystemWatcher            | 内置                                                   |
| 索引      | 内存索引                     | 目录扫描 + 路径/size/mtime 比对，仅新增或变动文件算哈希；watcher 增量更新 |
| 哈希      | Blake3                         |                                                        |
| 图像处理  | ImageSharp                   | 解码 JPEG/PNG/GIF/WebP/TIFF/BMP，缩放生成缩略图        |
| 配置      | Tomlyn                       | TOML 解析                                              |
| OpenAPI   | Microsoft.AspNetCore.OpenApi | 从代码生成 schema                                      |
| 日志      | ILogger（Serilog Provider）  |                                                        |

## 发布

`dotnet publish` 自包含单文件，按平台（win-x64 / osx-arm64 / linux-x64）产出 hawk-server 二进制，随 Electron 一起打包。NativeAOT 作为后续优化项，C# 阶段不启用。

## 图像格式扩展路线

MVP 用 ImageSharp 覆盖常见格式。图像解码收在一个接口后面，RAW、HEIC 等格式后续再定实现方案。

## 启动顺序

先监听、后索引（Syncthing 式启动模型，见 architecture.md）：Kestrel 立即开放端口，初始扫描在索引流水线后台构建，就绪语义由状态端点表达而非端口可达性。

1. 启动文件监听（事件先入缓冲队列）
2. 启动 Kestrel 监听（端口对客户端立即可达）
3. 后台扫描素材目录并加载 `metadata/`，按路径与 size/mtime 比对，仅对新增或变动的文件计算哈希；哈希确认后将最新 size/mtime 回写元数据
4. 重放缓冲事件，与加载结果合并去重
5. 索引完成 → `StartupState.MarkReady()`：`/health` 转 200、`/api/*` 网关放行；此前一切 API 返回 503 `NOT_READY`，进度经 `GET /api/v1/app/startup` 查询
6. 索引失败 → 进程保留，`/api/v1/app/startup` 返回 `error` 与原因（修复后重启）

## 并发模型

索引流水线（监听 → 哈希 → 缩略图 → 入库）用 `System.Threading.Channels` 串联，有界 channel 提供背压；HTTP 层由 ASP.NET Core 处理。细节：

- 索引与元数据写入收敛在单消费者循环（单写者）；监听事件幂等，channel 满时置溢出标记，由全量扫描兜底
- 全量扫描分两阶段：复用判定串行，哈希计算并行（`ProcessorCount/2`），索引应用串行
- 写入防抖：mtime 距今不足 1 秒的文件延迟重试（去重、上限 120 次），避免对拷贝中的大文件反复哈希
- 缩略图生成是独立后台 worker 池（1–4 并发），不阻塞索引

## 替换为 Rust 时的注意点

- 目标栈：axum、notify、blake3、image + fast_image_resize，索引同样在内存中维护
- 行为对齐以 OpenAPI schema 为准，不逐行翻译 C# 代码；启动握手（`app/startup` 轮询、就绪网关）属 schema 契约的一部分
- `.hawk/` 存储格式（metadata TOML、缩略图命名、trash 结构）属于持久化契约，Rust 版必须兼容

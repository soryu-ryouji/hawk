# hawk-server（C# 过渡实现）

> 逐文件的代码职责与流程串联见 [代码导读](server-code-structure.md)。

第一版后端使用 C# 实现，目的是验证 API 设计、跑通完整后端流程。验证完成后将整体替换为 Rust 实现。

替换对前端透明：前端只依赖 OpenAPI 契约，不感知后端语言。因此 C# 阶段就要保证 OpenAPI schema 的准确性和完整性，schema 即契约。

## 技术选型

| 职责      | 选型                         | 实现要点                                               |
| --------- | ---------------------------- | ------------------------------------------------------ |
| HTTP 框架 | ASP.NET Core Minimal API     |                                                        |
| 文件监听  | FileSystemWatcher            | 内置                                                   |
| 索引      | 内存索引                     | 目录扫描 + 路径/size/mtime 比对，仅新增或变动文件算哈希；watcher 增量更新；元数据经库外 SQLite 派生缓存注水（见 storage.md） |
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

先监听、后索引（Syncthing 式启动模型，见 architecture.md）：就绪语义由状态端点表达而非端口可达性。启动不再等待全库扫描——内存索引由 SQLite 派生缓存一次顺序读注水（`IndexPipeline.HydrateIndex`，回退 TOML 全量解析），秒级就绪。Kestrel **先**监听，索引装配/注水**后**行：注水或缓存重建期间 `/app/startup` 持续可答 starting，客户端始终有反馈：

1. 元数据副本注水：SQLite 缓存（快路径）→ TOML 全量解析（缓存缺失/重建时的一次性慢路径）
2. 内存索引注水：遍历元数据副本登记 hash → item/位置（宽高取派生缓存，调色板取 colors 缓存；回收站位置不在元数据中，由对账扫描登记）
3. `StartupState.MarkReady()`：`/health` 转 200、`/api/*` 网关放行；此前一切 API 返回 503 `NOT_READY`（注水本身是顺序读，通常亚秒级完成）
4. 启动文件监听（事件先入缓冲队列，消费循环随后重放，与对账扫描天然去重）
5. 后台全库对账扫描：按路径与 size/mtime 比对，仅对新增或变动的文件计算哈希；停机期间的增删改由此收敛（就绪到扫描完成之间有秒级~分钟级窗口，运行期变更由监听事件实时覆盖）
6. 周期对账（默认 60s）继续兜底文件监听静默丢事件

元数据本身就是哈希缓存（TOML 文件名即哈希，`paths` 记录校验依据），启动时无需读取文件内容。hawk 未运行期间对素材目录的改动由后台对账扫描自动发现；如对索引状态存疑，可手动调用 `POST /api/v1/library/reindex` 全量重建（见 API 文档）。
## 并发模型

索引流水线（监听 → 哈希 → 入库）用 `System.Threading.Channels` 串联，有界 channel 提供背压；HTTP 层由 ASP.NET Core 处理。缩略图/调色板在独立的 `ThumbnailWorker` 后台线程池处理，经回调回写流水线。细节：

- 索引与元数据写入收敛在单消费者循环（单写者）；监听事件幂等，channel 满时置溢出标记，由全量扫描兜底
- 全量扫描分两阶段：复用判定串行，哈希计算并行（`ProcessorCount/2`），索引应用串行
- 写入防抖：mtime 距今不足 1 秒的文件延迟重试（去重、上限 120 次），避免对拷贝中的大文件反复哈希；哈希完成后复验 size/mtime，仍在写入的延迟重试，不以半截内容入库（慢速拷贝来源的哈希漂移由此根治）
- 缩略图/调色板在独立的 `ThumbnailWorker` 后台线程处理（并发 `CPU/4`、封顶 8，`BelowNormal` 优先级让出 API 调度），不阻塞索引；同一 hash in-flight 去重，已齐备的文件（对账扫描重放）不产生任务

## 替换为 Rust 时的注意点

- 目标栈：axum、notify、blake3、image + fast_image_resize，索引同样在内存中维护
- 行为对齐以 OpenAPI schema 为准，不逐行翻译 C# 代码；启动握手（`app/startup` 轮询、就绪网关）属 schema 契约的一部分
- SSE 事件契约（事件名、负载形状、时序语义）见 API 文档 events 节，同为持久化契约
- `.hawk/` 存储格式（metadata TOML、缩略图命名、trash 结构）属于持久化契约，Rust 版必须兼容

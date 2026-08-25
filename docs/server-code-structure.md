# hawk-server 代码导读

面向新加入者的代码地图：每个文件做什么、与谁协作，以及几条关键流程如何把代码串起来。
设计背景见 [server-csharp.md](server-csharp.md)（技术选型与启动顺序）、[storage.md](storage.md)（存储格式）、[server-rest-api-v1.md](server-rest-api-v1.md)（接口契约）。

## 总览

 hawk-server 分两层，依赖单向：`Api/` → `Core/`。

```text
HTTP 请求 ──► Api/（端点、信封、鉴权）──► 读：ItemIndex 直查
                                        └► 写：真实文件操作 → IndexPipeline 提交任务并等待
文件系统变化 ──► LibraryWatcher ──► IndexPipeline（事件入队）
                                        │
                                        ▼
                              IndexPipeline（单写者消费者）
                                哈希 → 元数据迁移 → 更新 ItemIndex
                                → 写 MetadataStore → 派发缩略图 → EventBus 发 SSE
```

两条硬规则：

1. **索引与元数据的写入只发生在 IndexPipeline 的消费循环里**（单写者）。Api 层做真实文件操作（移动/复制），随后把变更作为任务提交给流水线并等待完成；HTTP 线程只读索引。
2. **流水线的所有处理幂等**。同一事件重复到达（例如 API 主动移动文件后，watcher 又上报一次）不会产生副作用，这是免锁设计成立的前提。

## 文件清单与职责

### 启动入口

| 文件 | 职责 |
| ---- | ---- |
| `src/Program.cs` | 组装与启动：参数解析、DI 注册、中间件、启动顺序、就绪信号 |

Program.cs 做的事（按执行顺序）：

1. `ServerSettings.FromArgs` 解析命令行与环境变量；`ResolvePort` 用 `TcpListener` 试绑 27371，被占用则返回 0 交给 Kestrel 动态分配
2. `WebApplicationBuilder`：绑定 `127.0.0.1`、Serilog 控制台日志、JSON 全局 snake_case + null 省略、`AddOpenApi`、CORS 全放开（localhost 工具，token 兜底）
3. 注册单例：`LibraryPaths`（注册时 `EnsureLayout` 创建 `.hawk/` 结构与 `.gitignore`）、`LibraryConfig`、`MetadataStore`（构造时加载全部元数据）、`ItemIndex`、`ThumbnailService`、`EventBus`、`LibraryScanner`、`IndexPipeline`、`LibraryWatcher`
4. 中间件顺序：CORS → `ErrorHandlingMiddleware` → `TokenAuthMiddleware`
5. **启动顺序**（对应 server-csharp.md）：`pipeline.Start()` → 接线 watcher 事件 → `watcher.Start()`（事件先入队缓冲）→ `await pipeline.RunScanAsync(false)` 阻塞至初始索引完成 → `app.StartAsync()` 此后 `/health` 才可达
6. 打印 `HAWK_READY <address> token=<token>` 行，Electron 主进程解析它拿到端口与 token
7. `ApplicationStopping` 时释放 watcher 与 pipeline

### Core/ —— 与 HTTP 无关的领域核心

| 文件 | 职责 |
| ---- | ---- |
| `ServerSettings.cs` | 启动设置：`--library` / `--port` 与 `HAWK_*` 环境变量解析；token 未传入时生成随机值（开发场景） |
| `LibraryPaths.cs` | 路径规则的唯一权威：`.hawk/` 布局、相对/绝对路径互转（越界防护）、`.hawk` 内部判定、回收站路径换算、文件名片段工具 |
| `LibraryConfig.cs` | `.hawk/config.toml`：解析为不可变快照（name / ignore / thumbnail_sizes），`Reload()` 由流水线在扫描前调用；ignore 模式无 `/` 时匹配任意深度同名项 |
| `ItemMetadata.cs` | 元数据模型：`ItemMetadata`（url/tags/star/annotation + `List<PathEntry>`），对应一个 `<hash>.toml` |
| `MetadataStore.cs` | 元数据存取：内存权威副本 + path→hash 反查表；磁盘原子写（临时文件 + rename）；只认 64 位小写 hex 文件名（同步冲突副本自动忽略）；TOML 解析用 Tomlyn，序列化手写以精确控制格式 |
| `ContentHash.cs` | BLAKE3 流式哈希 → hex，即 item id |
| `Item.cs` | 索引中的 item：`ItemLocation`（一个文件位置，回收站位置以 `.hawk/trash/` 开头）与 `Item`（位置列表 + 元数据副本 + 宽高派生信息）；`ToDto(trashView)` 负责向 API 投影——回收站视图的 paths 展示原库内路径（恢复目标） |
| `ItemIndex.cs` | 内存索引：hash→item 与位置路径→hash 两个字典，一把锁保护；`Query` 实现 item/list 的全部过滤（AND 语义）、排序与分页 |
| `ThumbnailService.cs` | ImageSharp 封装：`Identify` 只读头部取尺寸；`GenerateAsync` 按配置尺寸输出 WebP（`ResizeMode.Max` 等比缩小、不放大小图）；缩略图按 hash 内容寻址存储 |
| `LibraryScanner.cs` | 目录遍历：跳过 `.hawk/` 内部（只深入 trash 子树）、库内应用 ignore 规则、枚举失败静默跳过 |
| `LibraryWatcher.cs` | FileSystemWatcher 封装：Created/Changed→upsert、Deleted、Renamed→move、Error→溢出回调；过滤 `.hawk` 内部，config.toml 单独上报 |
| `EventBus.cs` | SSE 事件总线：每个订阅者一条有界 channel；消费跟不上就断开该订阅（前端重连后用 item/list 全量对齐） |
| `LibraryFs.cs` | 文件操作小工具：建父目录、回收站冲突时追加 ` (n)` 后缀、名称合法性校验 |
| `IndexPipeline.cs` | **核心**：索引流水线，详见下节 |

### IndexPipeline.cs 详解

所有索引变更收敛为一个单消费者流水线，任务经有界 channel（4096）串行处理：

- **任务类型**：`UpsertJob`（新增/变更，可携带 `KnownHash` 跳过流水线重算、供 item/add 复用 API 侧已算的哈希）、`DeleteJob`（按路径与目录前缀双重匹配，因为删除事件分不清文件还是目录）、`MoveJob` / `DirMoveJob`、`ScanJob`（full 时强制重算全部哈希）、`ClearTrashJob`、`MetadataJob`（item/update 的元数据写）
- **两类入口**：watcher 走 fire-and-forget，channel 满则置溢出标记，消费者每批任务后检查并触发全量扫描兜底；API 与启动走携带 `TaskCompletionSource` 的提交，等待处理完成后返回结果
- **写入防抖**：入库拆成 `PrepareUpsert`（路径过滤、复用判定，不读文件内容）与 `ApplyUpsert`（串行应用变更）两步。判定需要算哈希时，若文件 mtime 距今不足 1 秒（大文件仍在拷贝中），不立即处理，经去重集合延迟重试（上限 120 次）——避免对半截内容反复哈希；携带 KnownHash 或等待结果的提交不防抖（文件由 API 写入，内容已完整）
- **入库流程（ApplyUpsert）**：哈希漂移时按路径迁移元数据（新 item 继承 tags 等素材参数，旧元数据无引用则删除）；回写最新 size/mtime 保持校验依据新鲜；补读图像尺寸；发 `item.added` / `item.updated`；缩略图派发到后台 worker（1–4 个并发，CPU 密集不阻塞索引）
- **`DoMove` / `MoveOne`**：索引 rekey 不重算哈希；lib→lib 时元数据路径跟随，lib↔trash 时去掉前缀后库内路径不变，元数据保持原路径作为恢复目标；目录移动后补扫新位置，吸收 watcher 遗漏的子文件事件
- **`DoScan`（两阶段扫描）**：阶段一串行遍历做复用判定（命中元数据的直接应用，不读内容）；阶段二对需要哈希的文件并行计算（`ProcessorCount/2`，纯计算不碰共享状态）；阶段三串行应用结果并做消失检测。单写者模型不变，仅哈希计算并行
- **`DoClearTrash`**：摘除回收站位置并清理元数据路径；内容在库内无其他引用时删除元数据与缩略图
- **事件语义**：位置归零 → `item.removed`；只剩回收站位置 → `item.trashed`；首个库内位置回归 → `item.restored`；其余变化 → `item.updated`

### Api/ —— HTTP 层

| 文件 | 职责 |
| ---- | ---- |
| `Envelope.cs` | 统一信封（`Envelope<T>` / `ErrorEnvelope`）、错误码常量、`ApiException`（携带错误码与 HTTP 状态）、`ErrorHandlingMiddleware`（异常 → 错误信封，未知异常 500 INTERNAL） |
| `AuthMiddleware.cs` | `/api/*` 必须携带 `Authorization: Bearer <token>`；`/api/v1/events` 接受 `?token=`（EventSource 无法设请求头）；`/health`、`/openapi` 不在 `/api` 前缀下天然放行 |
| `AppEndpoints.cs` | `GET /health`（探活）、`GET /api/v1/app/info`（版本/平台/可执行路径） |
| `LibraryEndpoints.cs` | `library/info`（显示名取 config 的 name，缺省为目录名）、`library/reindex`（入队全量重哈希扫描，立即返回） |
| `FolderEndpoints.cs` | folder 五端点。folder 即真实目录：list 实时从文件系统建树（排除 `.hawk` 与 ignore 目录）；create/update/delete/restore 先做校验（名称合法、父目录存在、目标占用 → `FILE_EXISTS`、禁止移入自身子目录），再做真实目录操作，最后 `SubmitDirMoveAsync` 同步索引 |
| `ItemEndpoints.cs` | item 九端点，逻辑最重，见下节 |
| `TrashEndpoints.cs` | `trash/clear`：物理删除 `.hawk/trash/` 全部内容，再提交 `ClearTrashJob` 清理元数据与缩略图 |
| `EventsEndpoints.cs` | SSE 订阅端点：循环读 EventBus channel 写 `event:`/`data:` 帧；断连时注销订阅；流式响应不纳入 OpenAPI schema |

### ItemEndpoints.cs 详解

- **list / detail / count**：纯查询，直接读 `ItemIndex`。detail 的视图自动判断（item 无库内位置时按回收站视图投影）
- **add**：`path`/`url`/`img_base64` 三选一取内容（url 下载到临时文件，base64 必须能被 ImageSharp 识别否则 `UNSUPPORTED_FORMAT`）→ 推断扩展名与缺省文件名 → 目标已存在报 `FILE_EXISTS` → **写入前先算哈希**确定 `already_existed`（避免 watcher 竞态改变语义）→ 文件落库 → `SubmitUpsertAsync` 完成索引 → 附带的 tags/annotation/url 经 `SubmitMetadataAsync` 写入元数据
- **update**：定位操作位置（`path` 指定或主位置；回收站位置用原库内路径匹配）→ 回收站中的文件禁止改名/移动 → `name`/`folder_path` 做真实文件移动并 `SubmitMoveAsync` → tags/star/annotation/url 走 `SubmitMetadataAsync`（star 校验 0–5）
- **delete / restore**：delete 把文件移入 `.hawk/trash/`（保留目录结构，冲突加 ` (n)` 后缀）；restore 按回收站实际名称去掉前缀后的路径放回，被占用报 `FILE_EXISTS`
- **thumbnail**：尺寸必须在 `thumbnail_sizes` 白名单内；响应 `Cache-Control: immutable`（id 是内容哈希，内容永不变）；缩略图未生成时 404
- **refresh_thumbnail**：取一个可读位置（优先库内）强制重建全部尺寸

### 工程与测试文件

| 文件 | 说明 |
| ---- | ---- |
| `hawk-server.csproj` | net10.0；依赖：Blake3、SixLabors.ImageSharp、Tomlyn、Serilog.AspNetCore、Microsoft.Extensions.FileSystemGlobbing、Microsoft.AspNetCore.OpenApi |
| `appsettings*.json` | 仅保留默认日志级别配置；Serilog 目前在代码里配置控制台输出 |
| `tools/smoke.sh` | 端到端冒烟测试（46 项断言）：临时素材库 + curl 覆盖鉴权、索引、过滤、缩略图、去重、文件夹、监听、写入防抖、SSE、回收站全流程与重启后哈希复用。运行前先 `dotnet build` |

仓库根目录另有 `hawk-server.Tests/`（xunit 单元/集成测试），见下文「测试」一节。

## 关键流程串联

### 1. 启动建索引（对应 server-csharp.md 的启动顺序）

```text
watcher.Start()          事件开始入队（channel 缓冲）
    └► RunScanAsync(false)  阻塞：WalkLibrary 逐文件 DoUpsert
         路径+size/mtime 命中元数据 → 复用哈希（不读内容，仅 Identify 取尺寸）
         否则算哈希并登记元数据
    └► 扫描完成 → Kestrel 开始监听 → /health 200
```

缓冲的 watcher 事件排在扫描任务之后，被幂等处理自然去重。

### 2. 外部文件变更（用户在 Finder 里操作）

```text
LibraryWatcher → NotifyUpsert/NotifyDeleted/NotifyMoved
  → pipeline 入队 → 防抖（写入中文件延迟重试）→ 哈希/迁移/索引更新 → EventBus → SSE → 前端增量刷新
```

### 3. item/add

```text
Api: 取内容（复制/下载/解码）→ 预计算哈希 → 文件写入库内
   → SubmitUpsertAsync（携带已知哈希，流水线跳过重算）→ SubmitMetadataAsync（素材参数）
   → 返回 item + already_existed
watcher 随后上报的 Created 事件幂等吸收
```

### 4. 回收站三段操作

```text
item/delete:   File.Move → .hawk/trash/<原路径>（冲突加后缀）→ MoveJob
               元数据不动（paths 仍是原路径 = 恢复目标）
item/restore:  File.Move 回原路径（占用 → FILE_EXISTS）→ MoveJob
trash/clear:   物理删除 → ClearTrashJob 清位置、清元数据路径；
               内容无其他引用时删元数据与缩略图
```

## 测试

| 层 | 位置 | 说明 |
| ---- | ---- | ---- |
| 单元/集成测试 | `hawk-server.Tests/`（xunit，106 项） | Core 层纯逻辑（路径、元数据 TOML 往返、ignore 匹配、索引查询、BLAKE3 标准向量）+ IndexPipeline 临时目录集成测试（入库/哈希复用/id 漂移继承/移动/多路径/清空回收站/事件/防抖）。`dotnet test hawk-server.Tests` |
| 端到端契约测试 | `hawk-server/tools/smoke.sh`（46 项断言） | 临时素材库 + curl 覆盖 HTTP API 全流程；语言无关，未来 Rust 版可直接复用。运行前先 `dotnet build` |

测试策略：契约级测试（HTTP/存储格式）优先于内部单元测试——C# 版是过渡实现，Rust 重写后只有契约级测试能原样复用。

## 排查指引

- 索引不一致 → `POST /api/v1/library/reindex` 全量重建；watcher 缓冲溢出会自动触发同样的兜底扫描
- 缩略图 404 → 属首次索引期间的正常状态，否则调 `item/refresh_thumbnail`
- 行为对齐以 OpenAPI schema 为准（`/openapi/v1.json`），`.hawk/` 存储格式为持久化契约——未来 Rust 重写时必须兼容这两者

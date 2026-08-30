# hawk-server 代码导读

面向新加入者的代码地图：每个文件做什么、与谁协作，以及几条关键流程如何把代码串起来。
设计背景见 [server-csharp.md](server-csharp.md)（技术选型与启动顺序）、[storage.md](storage.md)（存储格式）、[server-rest-api-v1.md](server-rest-api-v1.md)（接口契约）。

## 总览

 hawk-server 分两层，依赖单向：`Api/` → `Core/`。

```text
HTTP 请求 ──► Api/（端点、信封、鉴权）──► 读：ItemIndex 锁内投影（DTO / 不可变快照）
                                        └► 写：真实文件操作 → IndexPipeline 提交任务并等待
文件系统变化 ──► LibraryWatcher ──► IndexPipeline（事件入队）
                                        │
                                        ▼
                              IndexPipeline（单写者消费者）
                                哈希 → 元数据迁移 → 更新 ItemIndex
                                → 写 MetadataStore → 派发缩略图 → EventBus 发 SSE
                                        │
                    ┌───────────────────┼────────────────────┐
                    ▼                   ▼                    ▼
            TaxonomyMigrator     ThumbnailWorker        ItemEvents
            分类/标签级联迁移    缩略图/调色板队列       事件名常量 + 发布辅助
            与元数据写应用      （CPU 密集，后台线程）    （负载契约见 API 文档 events 节）
```

三条硬规则：

1. **索引与元数据的写入只发生在 IndexPipeline 的消费循环里**（单写者）。Api 层做真实文件操作（移动/复制），随后把变更作为任务提交给流水线并等待完成；HTTP 线程只读索引。
2. **流水线的所有处理幂等**。同一事件重复到达（例如 API 主动移动文件后，watcher 又上报一次）不会产生副作用，这是免锁设计成立的前提。
3. **锁内投影，锁外只见 DTO**：`ItemIndex.Get` 返回可变 `Item` 引用，仅限流水线（单写者）与测试；HTTP 层一律走 `GetDto`（锁内投影）/ `Query` / `FindLocation`（不可变快照），锁外不得持有或遍历 `Item`。

## 文件清单与职责

### 启动入口

| 文件 | 职责 |
| ---- | ---- |
| `src/Program.cs` | 组装与启动：参数解析、DI 注册、中间件、启动顺序（先监听、索引后台构建）、就绪信号 |

Program.cs 做的事（按执行顺序）：

1. `ServerSettings.FromArgs` 解析命令行与环境变量；`ResolvePort` 用 `TcpListener` 试绑 27371，被占用则返回 0 交给 Kestrel 动态分配
2. `WebApplicationBuilder`：绑定 `127.0.0.1`、Serilog 控制台日志、JSON 全局 snake_case + null 省略、`AddOpenApi`、CORS 全放开（localhost 工具，token 兜底）
3. 注册单例：`LibraryPaths`（注册时 `EnsureLayout` 创建 `.hawk/` 结构与 `.gitignore`）、`LibraryConfig`、`IndexDb`（元数据 SQLite 派生缓存，见 storage.md）、`MetadataStore`（构造时注水元数据副本：缓存优先，TOML 全量解析回退）、`ItemIndex`、`ThumbnailService`、`ColorService`、`ThumbnailWorker`、`EventBus`、`LibraryScanner`、`TaxonomyMigrator`、`IndexPipeline`、`LibraryWatcher`
4. 中间件顺序：CORS → `ErrorHandlingMiddleware` → `TokenAuthMiddleware` → `ReadyGateMiddleware`（初始索引完成前拦截 `/api/*` 为 503 `NOT_READY`，仅放行 `/api/v1/app/startup`）
5. **启动顺序**（先监听、后索引，对应 server-csharp.md）：`pipeline.OnScanProgress` 装配 → `pipeline.AttachThumbnailWorker(worker)`（缩略图回调闭环：索引投影 + PaletteJob 回写）→ `pipeline.Start()`（内部先 `HydrateIndex` 从元数据副本注水喝入内存索引，再启动消费循环与 ThumbnailWorker 线程）→ `startup.MarkReady()`（`/health` 转 200、网关放行——不再等待全库扫描）→ 接线 watcher 事件（文件增删改移 + 目录创建 `FolderCreated` → `NotifyFolderChanged`）→ `watcher.Start()`（事件先入队缓冲）→ `RunScanAsync(false)` 作为后台对账任务（失败仅记日志，周期对账兜底）。`StartupState` 单例汇总 `OnScanProgress` 进度；`Fail()` 仅注水阶段使用（进程保留，错误经 `app/startup` 暴露）
6. 日志打印监听地址与就绪状态；握手无 stdout 私有协议——端口由 Electron 预选传入，进度/就绪经 `/api/v1/app/startup` 轮询
7. `ApplicationStopping` 时释放 watcher 与 pipeline

### Core/ —— 与 HTTP 无关的领域核心

| 文件 | 职责 |
| ---- | ---- |
| `ServerSettings.cs` | 启动设置：`--library` / `--port` 与 `HAWK_*` 环境变量解析；token 未传入时生成随机值（开发场景） |
| `LibraryPaths.cs` | 路径规则的唯一权威：`.hawk/` 布局、库外派生缓存目录（`%LOCALAPPDATA%/hawk/cache/<库文件夹名>_<哈希16位>/`，`cacheDir` 参数供测试覆盖）、相对/绝对路径互转（越界防护）、`.hawk` 内部判定、回收站路径换算、文件名片段工具 |
| `LibraryConfig.cs` | `.hawk/config.toml`：解析为不可变快照（name / ignore / thumbnail_sizes），`Reload()` 由流水线在扫描前调用；ignore 模式无 `/` 时匹配任意深度同名项 |
| `ItemMetadata.cs` | 元数据模型：`ItemMetadata`（url/tags/star/annotation + `List<PathEntry>`），对应一个 `<hash>.toml` |
| `IndexDb.cs` | 元数据 SQLite 派生缓存（库外缓存目录 `index.db`）：TOML 的纯镜像（items 含宽高/调色板字段 / paths / tags / categories + source_mtime），journal_mode=DELETE——提交直接写主库文件，不产生 -wal/-shm 伴生文件（本进程全部 db 访问都在流水线单线程，WAL 的读写并发优势用不上）。注水（Hydrate，事务内置 hydrated 标记，空库也是合法完成态）/全量读（LoadAll）/写穿（Save/Delete）/源 mtime 快照（LoadSourceMtimes，对账比对依据）。打开失败或写失败熔断退化为纯 TOML 模式——缓存故障绝不影响权威数据；schema 版本不符直接重建（项目未发布，缓存是派生数据，无历史格式兼容义务）。宽高/调色板随 items 镜像读写（启动注水一次顺序读）；文件夹快照表 folders 供增量扫描。设计契约见 [storage.md](storage.md)「元数据缓存」 |
| `MetadataStore.cs` | 元数据存取：内存权威副本 + path→hash 反查表。TOML 原子写（临时文件 + rename）；**写入顺序铁律：先 TOML 成功后再写 IndexDb 与内存**，崩溃朝 TOML 收敛；副本注水来源：IndexDb 快路径 → TOML 全量解析回退（顺带建缓存）。对账入口：`ApplyExternalToml`（外部新增/变更载入，解析失败跳过不清空）、`ClearExternal`（TOML 消失清空素材参数）、`SourceMtimes`。只认 64 位小写 hex 文件名（同步冲突副本自动忽略）；TOML 解析用 Tomlyn，序列化手写以精确控制格式 |
| `ContentHash.cs` | BLAKE3 流式哈希 → hex,即 item id;`FileShare.ReadWrite\|Delete` 共享读——哈希计算期间不阻塞文件的移动/删除 |
| `Item.cs` | 索引中的 item:`ItemLocation`(一个文件位置,回收站位置以 `.hawk/trash/` 开头)与 `Item`(位置列表 + 元数据副本 + 宽高派生信息);`ToDto(trashView)` 负责向 API 投影——回收站视图的 paths 展示原库内路径(恢复目标);`SyncFrom(meta)` 为元数据 → 索引的单向同步(只允许流水线调用) |
| `ItemIndex.cs` | 内存索引:hash→item 与位置路径→hash 两个字典,一把锁保护。`Query` 实现 item/list 的全部过滤(AND 语义)、排序与分页;`QuerySkeleton` 同过滤排序投影 dim 全量;排序主键同值按 id 打破平局。**读取纪律**:`Get` 返回可变引用,仅限流水线(单写者)与测试;HTTP 层走 `GetDto`(锁内投影)/`Contains`/`FindLocation`/`MainSourceAbs`(不可变快照) |
| `ItemEvents.cs` | SSE 事件名常量(`item.added`/`updated`/`trashed`/`restored`/`removed`、`folder.changed`、`task.progress`)与发布辅助(`PublishChanged`/`PublishLocationLoss`/`PublishTransition`);负载字段契约见 API 文档 events 节 |
| `ThumbnailWorker.cs` | 缩略图/调色板后台 worker:独立有界队列 + `CPU/4`(封顶 8)个专用后台线程(`BelowNormal` 优先级,与 Kestrel 同进程时让出调度;生成完成回调取 DTO 补发 `item.updated`,调色板提炼完成经回调回流水线 `PaletteJob`(单写者写入元数据 TOML 并同步索引);`inflight` 去重——同一 hash 队列中/生成中时重复派发丢弃;`Backlog` 快照供 `task.progress` 与 `app/status`;队列满丢弃(尽力而为的缓存) |
| `TaxonomyMigrator.cs` | 分类/标签级联迁移与元数据写应用:`ApplyMetadata`/`ApplyMetadataBatch`(batch_update)逐个 mutate → 落盘 → 同步索引 → 发事件;`RenameCategory`/`DeleteCategory`/`RenameTag`/`DeleteTag` 全库批迁移;注册表登记与外部改动重载 |
| `ThumbnailService.cs` | ImageSharp 封装:`Identify` 只读头部取尺寸;`GenerateAsync` 按配置尺寸输出 WebP(`ResizeMode.Max` 等比缩小、不放大小图);缩略图按 hash 内容寻址存储;文件一律共享读打开(`FileShare.ReadWrite\|Delete`)——缩略图生成可跨秒持有句柄,不阻塞源文件的移动/删除 |
| `ColorMath.cs` | 颜色纯函数：hex 解析/格式化、sRGB→CIELAB（D65）、CIE76 ΔE² 距离 |
| `ColorService.cs` | 调色板提炼（降采样 64px → Wu 量化 ≤10 色 → 像素占比，alpha<128 不参与）。提炼结果作为内容的纯函数写入素材元数据 TOML（参与同步，一台计算全平台复用），本类无缓存职责；检索原理见 [color-search.md](color-search.md) |
| `LibraryScanner.cs` | 目录遍历：跳过 `.hawk/` 内部（只深入 trash 子树）、库内应用 ignore 规则；目录枚举失败经 `onEnumerationError` 回调上报——调用方据此判定遍历不完整（DoScan 跳过消失对账，避免误删） |
| `LibraryWatcher.cs` | FileSystemWatcher 封装:Created(目录单独走 `FolderCreated` 事件驱动 folder.changed)/Changed→upsert、Deleted、Renamed→move、Error→溢出回调;过滤 `.hawk` 内部,config.toml 与注册表文件单独上报;另有周期对账扫描(`HAWK_RESCAN_INTERVAL`,默认 60s)兜底静默丢事件 |
| `EventBus.cs` | SSE 事件总线:每个订阅者一条有界 channel;消费跟不上就断开该订阅(前端重连后用 item/skeleton + folder/list 全量对齐) |
| `StartupState.cs` | 启动状态：进度快照（Phase/Processed/Total，含元数据对账 sync 相）、就绪标志、失败原因；`/health`、就绪网关与 `app/startup` 端点的共同数据源。就绪 = 内存索引注水完成（秒级），全库对账在其后后台进行 |
| `Taxonomy.cs` | 分类/标签维度：`CategoryName` 名称校验（扁平，无层级）；`CategoryRegistry` / `TagRegistry` 注册表（`.hawk/categories.toml`、`.hawk/tags.toml`，原子写）；支持空分类/空标签预创建，赋值时自动登记 |
| `ViewPreferences.cs` | 视图偏好注册表（`.hawk/view.toml`，参与同步）：记住 folder/category/tag 视图各自的排序。扁平 map（scope 键 `folder:\u003c路径\u003e`/`category:\u003c名\u003e`/`tag:\u003c名\u003e`），**不理解继承语义**（前端沿父链解析）；scope/排序值校验与归一化；文件夹移动/删除时 `folder:` 键由流水线调用 RenamePrefix/DeletePrefix 跟随清理；外部修改经 watcher 触发 Reload |
| `LibraryFs.cs` | 文件操作小工具：建父目录、回收站冲突时追加 ` (n)` 后缀、名称合法性校验 |
| `IndexPipeline.cs` | **核心**:索引流水线,详见下节 |

### IndexPipeline.cs 详解

所有索引变更收敛为一个单消费者流水线，任务经有界 channel（4096）串行处理：

- **任务类型(分类/标签)**:分类/标签的注册表与级联迁移全部委托 `TaxonomyMigrator`(本文件只保留 job 定义与分发):`CategoryCreateJob` / `CategoryUpdateJob` / `CategoryDeleteJob`、`TagCreateJob` / `TagUpdateJob` / `TagDeleteJob`;`RegistryReloadJob`(外部同步改动注册表文件时经 migrator 重载)
- **任务类型**:`UpsertJob`(新增/变更,可携带 `KnownHash` 跳过流水线重算、供 item/add 复用 API 侧已算的哈希)、`DeleteJob`(按路径与目录前缀双重匹配,因为删除事件分不清文件还是目录)、`MoveJob` / `DirMoveJob`、`ScanJob`(full 时强制重算全部哈希)、`MetadataSyncJob`(元数据对账,见下)、`ClearTrashJob`、`MetadataJob`(item/update 的元数据写)、`BatchMetadataJob`(item/batch_update 批量元数据,不存在的 id 记入 MissingIds)、`FolderHintJob`(广播 `folder.changed`,fire-and-forget)
- **两类入口**：watcher 走 fire-and-forget，channel 满则置溢出标记，消费者检查到后入队去重的 ScanJob（扫描被消费前不重复排队）——不内联扫描，避免事件风暴期反复全库遍历；API 与启动走携带 `TaskCompletionSource` 的提交，等待处理完成后返回结果
- **写入防抖**：入库拆成 `PrepareUpsert`（路径过滤、复用判定，不读文件内容）与 `ApplyUpsert`（串行应用变更）两步。判定需要算哈希时，若文件 mtime 距今不足 1 秒（大文件仍在拷贝中），不立即处理，经去重集合延迟重试（上限 120 次）——避免对半截内容反复哈希；携带 KnownHash 或等待结果的提交不防抖（文件由 API 写入，内容已完整）。**哈希完成后复验 size/mtime**：仍在写入的（慢速拷贝来源 writes 间隔可超过 1 秒）延迟重试，不以半截内容入库——否则拷贝完成后哈希漂移，表现为素材计数先降后升
- **入库流程(ApplyUpsert)**:哈希漂移时按路径迁移元数据(新 item 继承 tags 等素材参数,旧元数据无引用则删除);回写最新 size/mtime 保持校验依据新鲜;补读图像尺寸;加载调色板缓存(缺失由 ThumbnailWorker 补齐);发 `item.added` / `item.updated`;`UpsertResult` 直接携带锁内投影的 DTO(API 响应与事件同源);缩略图派发到 ThumbnailWorker(见该文件)
- **`DoMove` / `MoveOne`**:索引 rekey 不重算哈希;lib→lib 时元数据路径跟随,lib↔trash 时去掉前缀后库内路径不变,元数据保持原路径作为恢复目标;目录移动后广播 `folder.changed` 并补扫新位置,吸收 watcher 遗漏的子文件事件
- **`DoScan`(两阶段扫描)**:阶段一串行遍历做复用判定(命中元数据的直接应用,不读内容);阶段二对需要哈希的文件并行计算(`ProcessorCount/2`,纯计算不碰共享状态,哈希后复验 size/mtime、仍在写入的延迟重试);阶段三串行应用结果并做消失检测——遍历不完整(部分目录枚举失败)时跳过消失对账,避免误删已索引位置。单写者模型不变,仅哈希计算并行。全程经 `OnScanProgress` 上报进度(`scan`/`hash`/`apply`/`done` 四相、150ms 节流,由 `StartupState` 汇总后经 `/api/v1/app/startup` 提供给客户端),同时经 `task.progress`(`task=index`,500ms 节流)推送给已就绪客户端,`IndexProgress()` 快照供 `/api/v1/app/status`;完成后广播一次 `folder.changed`(外部删空目录等不产生任何事件,对账扫描是目录结构变化的兜底)
- **`DoMetadataSync`(元数据对账,只进不出)**:`.hawk/metadata/` 的 TOML 是唯一权威源(参与网盘同步),本机 SQLite 缓存与内存副本经此跟随外部变更。按文件 mtime 与 `IndexDb.source_mtime` 比对,只有变化的文件才重解析;新增/变更 → `MetadataStore.ApplyExternalToml` 载入后刷新索引副本、登记注册表、发 `item.updated`;TOML 消失 → `ClearExternal` 清空素材参数(item 与位置由扫描决定存续);解析失败跳过且不清空状态,下轮重试。启动时 `Start()` 先入队一轮(先于初始扫描),此后跟随周期对账(60s)——保证扫描做迁移继承时拿到的是最新元数据。本机写入在 Save 时已同步缓存,对账轮通常为 no-op
- **`DoClearTrash`**:摘除回收站位置并清理元数据路径;内容在库内无其他引用时删除元数据与缩略图
- **事件语义**(发布辅助集中在 `ItemEvents`):位置归零 → `item.removed`;只剩回收站位置 → `item.trashed`;首个库内位置回归 → `item.restored`;其余变化 → `item.updated`

### Api/ —— HTTP 层

| 文件 | 职责 |
| ---- | ---- |
| `Envelope.cs` | 统一信封（`Envelope<T>` / `ErrorEnvelope`）、错误码常量、`ApiException`（携带错误码与 HTTP 状态）、`ErrorHandlingMiddleware`（异常 → 错误信封，未知异常 500 INTERNAL） |
| `AuthMiddleware.cs` | `/api/*` 必须携带 `Authorization: Bearer <token>`；`/api/v1/events` 接受 `?token=`（EventSource 无法设请求头）；`/health`、`/openapi` 不在 `/api` 前缀下天然放行 |
| `AppEndpoints.cs` | `GET /health`(探活:索引完成前 503)、`GET /api/v1/app/startup`(启动状态 starting/ready/error + 进度)、`GET /api/v1/app/status`(后台任务积压:缩略图 `ThumbnailWorker.Backlog` + 索引 `IndexPipeline.IndexProgress`)、`GET /api/v1/app/info`(版本/平台/可执行路径) |
| `LibraryEndpoints.cs` | `library/info`(显示名取 config 的 name,缺省为目录名)、`library/reindex`(入队全量重哈希扫描,立即返回) |
| `FolderEndpoints.cs` | folder 五端点。folder 即真实目录:list 实时从文件系统建树(排除 `.hawk` 与 ignore 目录);create/update/delete/restore 先做校验(名称合法、父目录存在、目标占用 → `FILE_EXISTS`、禁止移入自身子目录),再做真实目录操作;create 完成后 `NotifyFolderChanged`,update/delete/restore 经 `SubmitDirMoveAsync` 同步索引(DirMoveJob 内广播 folder.changed) |
| `ItemEndpoints.cs` | item 十二端点，逻辑最重，见下节 |
| `TrashEndpoints.cs` | `trash/clear`：先提交 `ClearTrashJob` 清理索引位置、元数据与缓存（缩略图/调色板），再物理删除 `.hawk/trash/` 内容。顺序不能颠倒：先物理删除会让 watcher 的 Deleted 事件抢先摘除位置，DoClearTrash 找不到位置导致元数据与缓存泄漏（Windows 上 watcher 延迟低必现） |
| `EventsEndpoints.cs` | SSE 订阅端点：循环读 EventBus channel 写 `event:`/`data:` 帧；断连时注销订阅；流式响应不纳入 OpenAPI schema |
| `ViewEndpoints.cs` | view 三端点：`preferences`（全量 map）/`preference` PUT/DELETE。偏好与索引/元数据无耦合，端点直接读写注册表（自带锁），不经过索引流水线 |

### ItemEndpoints.cs 详解

- **list / skeleton / detail / count**：纯查询，直接读 `ItemIndex`（list/skeleton 走锁内 Query，detail 走 `GetDto` 锁内投影）。list 与 skeleton 共用 `BuildQuery`（同过滤同排序，主键同值按 id 打破平局——前端按 offset 取视口窗口依赖次序逐位一致）；skeleton 不分页、投影 id/width/height/star，供前端虚拟网格建完整布局
- **add**：`path`/`url`/`img_base64` 三选一取内容（url 下载到临时文件，base64 必须能被 ImageSharp 识别否则 `UNSUPPORTED_FORMAT`）→ 推断扩展名与缺省文件名 → 目标已存在报 `FILE_EXISTS` → **写入前先算哈希**确定 `already_existed`（避免 watcher 竞态改变语义）→ 文件落库 → `SubmitUpsertAsync` 完成索引 → 附带的 tags/annotation/url 经 `SubmitMetadataAsync` 写入元数据 → 响应取 `GetDto` 最新投影
- **update**：经 `FindLocation` 取位置快照（`path` 指定或主位置；回收站位置用原库内路径匹配）→ 回收站中的文件禁止改名/移动 → `name`/`folder_path` 做真实文件移动并 `SubmitMoveAsync`（name 分支后重查最新位置，改名+移动同请求时基于新文件名计算目标）→ tags/star/annotation/url 走 `SubmitMetadataAsync`（star 校验 0–5）→ 响应 `GetDto` 投影
- **batch_update**：批量入口，语义见 API 文档。校验后先逐个移动主位置（冲突/回收站项跳过并记入 missing），再 `SubmitBatchMetadataAsync` 一次提交元数据（标签/分类并集、评分设置）；不存在的 id 由流水线记入 MissingIds；响应 `{ updated, missing_ids }`
- **delete / restore**：delete 把文件移入 `.hawk/trash/`（保留目录结构，冲突加 ` (n)` 后缀）；restore 按回收站实际名称去掉前缀后的路径放回，被占用报 `FILE_EXISTS`；位置定位均走 `FindLocation` 快照
- **thumbnail**：尺寸必须在 `thumbnail_sizes` 白名单内；响应 `Cache-Control: immutable`（id 是内容哈希，内容永不变）；缩略图未生成时 404
- **file**：原图二进制，`MainSourceAbs` 取主位置（优先非回收站）的绝对路径，Content-Type 按扩展名推断；同样 immutable 缓存；与 thumbnail 一样放行查询参数 token（`<img>` 直链）
- **refresh_thumbnail**：`MainSourceAbs` 取可读位置（优先库内）强制重建全部尺寸

### 工程与测试文件

| 文件 | 说明 |
| ---- | ---- |
| `hawk-server.csproj` | net10.0；依赖：Blake3、SixLabors.ImageSharp、Tomlyn、Serilog.AspNetCore、Microsoft.Data.Sqlite（元数据缓存，原生库随自包含发布打包）、Microsoft.Extensions.FileSystemGlobbing、Microsoft.AspNetCore.OpenApi |
| `appsettings*.json` | 仅保留默认日志级别配置；Serilog 目前在代码里配置控制台输出 |
| `tools/smoke.sh` | 端到端冒烟测试（82 项断言）：临时素材库 + curl 覆盖鉴权、索引、过滤、颜色检索、缩略图、去重、文件夹、监听、写入防抖、SSE（item.updated / folder.changed）、batch_update、回收站全流程与重启后哈希复用。库外缓存路径按平台计算、库标识以 server 报告的库根路径换算（Windows 下 argv 路径经 MSYS 转换，与 shell 变量可能不同）。JSON POST 体一律经 stdin 传递（post_json），避免 Windows Git Bash 把 argv 中的中文转成 GBK。运行前先 `dotnet build` |

仓库根目录另有 `hawk-server.Tests/`（xunit 单元/集成测试），见下文「测试」一节。

## 关键流程串联

### 1. 启动建索引（对应 server-csharp.md 的启动顺序）

```text
watcher.Start()          事件开始入队（channel 缓冲）
    └► RunScanAsync(false)  阻塞：WalkLibrary 逐文件 DoUpsert
         路径+size/mtime 命中元数据 → 复用哈希（不读内容，仅 Identify 取尺寸）
         否则算哈希并登记元数据
    └► Kestrel 先监听（app/startup 可查询进度，/api/* 503 NOT_READY）
    └► HydrateIndex 注水完成 → MarkReady → /health 200、API 放行（秒级）
    └► 后台对账扫描 → 停机期间增删改收敛（task.progress(index) 可见）
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
| 单元/集成测试 | `hawk-server.Tests/`（xunit，208 项） | Core 层纯逻辑（路径、元数据 TOML 往返、IndexDb 注水/写穿/清空、视图偏好注册表、ignore 匹配、索引查询、颜色数学/调色板提炼、BLAKE3 标准向量）+ IndexPipeline 临时目录集成测试（入库/哈希复用/id 漂移继承/移动/多路径/清空回收站/调色板缓存/事件/防抖/批量元数据/元数据对账/排序偏好跟随目录移动/folder.changed）。`dotnet test hawk-server.Tests` |
| 端到端契约测试 | `hawk-server/tools/smoke.sh`（82 项断言） | 临时素材库 + curl 覆盖 HTTP API 全流程；语言无关，未来 Rust 版可直接复用。运行前先 `dotnet build` |

测试策略：契约级测试（HTTP/存储格式）优先于内部单元测试——C# 版是过渡实现，Rust 重写后只有契约级测试能原样复用。

## 排查指引

- 索引不一致 → `POST /api/v1/library/reindex` 全量重建；watcher 缓冲溢出会自动触发同样的兜底扫描
- 缩略图 404 → 属首次索引期间的正常状态，否则调 `item/refresh_thumbnail`
- 行为对齐以 OpenAPI schema 为准（`/openapi/v1.json`），`.hawk/` 存储格式与 SSE 事件契约（事件名、负载形状）为持久化契约——未来 Rust 重写时必须兼容这三者

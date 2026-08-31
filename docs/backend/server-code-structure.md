# hawk-server 代码导读

面向新加入者的代码地图：`hawk-server-rs/src/` 每个文件做什么、与谁协作，以及几条关键流程如何把代码串起来。
设计背景见 [server-rust.md](server-rust.md)（技术选型与状态）、[storage.md](storage.md)（存储格式）、
[server-rest-api-v1.md](server-rest-api-v1.md)（接口契约）、[architecture.md](../architecture.md)（进程模型）。

## 总览

hawk-server 分两层，依赖单向：`api/` → `core/`。HTTP 请求不直接碰可变索引，
写路径一律把变更作为 Job 提交给索引流水线并等待完成。

```text
HTTP 请求 ──► api/（端点、信封、鉴权中间件）──► 读：ItemIndex 锁内投影（DTO / 骨架）
                                        └► 写：真实文件操作 → IndexPipeline 提交任务并等待
文件系统变化 ──► LibraryWatcher ──► IndexPipeline（Job 入队，有界 channel 4096）
                                        │（专用消费线程，单写者）
                                        ▼
                              IndexPipeline（消费循环）
                                哈希 → 元数据迁移 → 更新 ItemIndex
                                → 写 MetadataStore → 派发缩略图 → EventBus 发 SSE
                                        │
                    ┌───────────────────┼────────────────────┐
                    ▼                   ▼                    ▼
            TaxonomyMigrator     ThumbnailWorker        ItemEvents
            分类/标签级联迁移    缩略图/调色板队列       事件名常量 + 发布辅助
            与元数据写应用      （CPU 密集，专用线程池）
```

三条硬规则：

1. **索引与元数据的写入只发生在 IndexPipeline 的消费循环里**（单写者）。Api 层做真实文件操作（移动/复制），随后把变更作为 Job 提交给流水线并等待完成；HTTP 线程只读索引。
2. **流水线的所有处理幂等**。同一事件重复到达（例如 API 主动移动文件后，watcher 又上报一次）不会产生副作用，这是免锁设计成立的前提。
3. **锁内投影，锁外只见 DTO**：`ItemIndex` 一把锁保护，`with_item_mut` 可变访问仅限流水线；HTTP 层一律走 `get_dto`（锁内投影）/ `query` / `find_location` / `main_source_abs`（不可变快照）。

## 文件清单与职责

### 启动入口

| 文件 | 职责 |
| ---- | ---- |
| `src/main.rs` | 组装与启动（按执行顺序）：`Settings::from_args` 解析参数 → `resolve_port` 试绑 27371（占用回退动态分配）→ `resolve_lan_binding`（[web] 启用且配 token 时追加 0.0.0.0 绑定，端口占用直接启动失败）→ 构建全部单例 → `build_router` → **先监听**（环回 + 可选 LAN，`axum::serve` 优雅退出）→ `pipeline.start()`（注水/消费线程/worker/周期对账）→ 接线 watcher → `startup.mark_ready()` → 后台全库对账扫描 |
| `src/settings.rs` | 启动设置：`--library` / `--port` / `--web-dist` 与 `HAWK_*` 环境变量解析；库目录不存在 exit(2)；token 未传入时生成随机值并打印 stdout（开发场景）；`HAWK_RESCAN_INTERVAL` 周期对账间隔（默认 60s，0 关闭） |

### api/ —— HTTP 层

| 文件 | 职责 |
| ---- | ---- |
| `src/api/mod.rs` | `AppState` 共享状态与 `build_router`；中间件链 cors → auth → ready_gate（axum 中后注册的 layer 在外层，请求依次经过）；auth 双 token：admin（进程 token，全权限）与 viewer（config.toml `[web].token`，只读：GET + item/list、skeleton），`/api/v1/events`、`item/thumbnail`、`item/file` 三个无法设请求头的 GET 端点放行 `?token=`；ready_gate 未就绪时 `/api/*` 一律 503 `NOT_READY`，仅放行 `app/startup` |
| `src/api/envelope.rs` | 统一成功/错误信封、错误码常量、`ApiError` → 响应的 `IntoResponse`；`JsonBody` 提取器把 JSON 解析失败统一转为 `INVALID_PARAM` |
| `src/api/app.rs` | `health`（就绪前 503，无需 token）、`startup`（starting 带进度 / ready / error）、`status`（缩略图与索引积压快照，轮询型客户端用）、`info`（版本/平台/可执行路径/access 级别）、`token`（token 发现：Host 限定环回地址防 DNS rebinding，响应不带 CORS 头） |
| `src/api/library.rs` | `library/info`（显示名取 config 的 name，缺省目录名）、`reindex`（全量重哈希扫描，异步立即返回）、`rescan`（忽略目录快照强制遍历，仍按 size/mtime 复用哈希） |
| `src/api/folder.rs` | folder 五端点。list **实时从文件系统建树**（排除 `.hawk` 与 ignore 目录，count 取索引的 `folder_counts`，字典序不区分大小写）；create/update/delete/restore 先做校验（名称合法、父目录存在、`FILE_EXISTS`、禁止移入自身子目录）再做真实目录操作，变更经 `submit_dir_move` 同步索引（DirMoveJob 内广播 folder.changed，端点不重复通知） |
| `src/api/taxonomy.rs` | category/tag 八端点。list = 注册表 ∪ 全部 item 赋值并集（count 不含回收站）；名称校验在端点层，写全部 `submit_*` 给流水线执行 |
| `src/api/view.rs` | view 三端点（preferences / preference PUT / DELETE）。偏好与索引/元数据无耦合，注册表自带锁，端点直接读写，**不经过索引流水线** |
| `src/api/trash.rs` | `trash/clear`。顺序铁律：先 `submit_clear_trash` 清索引位置、元数据与缓存（缩略图/调色板），再物理删除 `.hawk/trash/` 内容——先物理删除会让 watcher 的 Deleted 事件抢先摘除位置，导致元数据与缓存泄漏 |
| `src/api/events.rs` | SSE 订阅端点：`broadcast::Receiver` 转 `event:`/`data:` 帧；lagged（消费跟不上）或总线关闭即结束流，客户端重连后须以 `item/skeleton` + `folder/list` 全量对齐 |
| `src/api/openapi.rs` | `/openapi/v1.json` 静态服务（`include_str!` 固化 `hawk-server-rs/openapi.json`），schema 即契约，不随后端实现漂移 |
| `src/api/web_dist.rs` | 局域网 web 查看的静态托管（fallback 挂载；`--web-dist` 传入时启用）：SPA 回退 `index.html`，`/assets/` 内容哈希资源 immutable 长缓存，其余 no-cache（防手机浏览器启发式缓存旧 HTML） |
| `src/api/item.rs` | item 十三端点，逻辑最重，见下节 |

### core/ —— 与 HTTP 无关的领域核心

| 文件 | 职责 |
| ---- | ---- |
| `src/core/paths.rs` | 路径规则的唯一权威：`.hawk/` 布局与 `ensure_layout`（含给 `.hawk/.gitignore` 补 `trash/` 排除项）、库内相对路径 ↔ 绝对路径互转（`..`/越界/绝对路径一律拒绝）、`is_internal`（`.hawk` 内部，回收站除外）、回收站前缀换算、`is_valid_library_path`（API 入参守卫）、库外缓存目录命名（`<库文件夹名>_<根路径 SHA-256 前16位>`，同名库靠哈希区分）与旧版纯哈希目录的一次性迁移、纯路径工具（`full_path` 文本归约、`dir_of`/`name_of`/`ext_of`、`unix_ms`/`file_mtime_ms`） |
| `src/core/config.rs` | `.hawk/config.toml` 快照（RwLock）与热更 `reload`（watcher 触发）；库首次打开生成带注释的默认配置；ignore 匹配器：无 `/` 的模式展开为 `**/p` + `**/p/**`（任意深度同名），分段 glob（`**` 跨目录、段内 `*`/`?`），大小写不敏感；`peek_web` 供启动期静态读取 `[web]` 段（LAN 绑定决策） |
| `src/core/index_db.rs` | 元数据 SQLite 派生缓存（库外 `index.db`）：schema v1（`items`/`paths`/`tags`/`categories` 四表 + `folders` 快照表幂等补建 + `meta`），`journal_mode=DELETE` 不产生 -wal/-shm，`busy_timeout` 5s；`hydrated` 注水标记（false 时内容不可信，必须由 TOML 全量重建）；打开失败/写失败/读失败一律 **poison 熔断**，退化为纯 TOML 模式——缓存故障绝不影响权威数据；`load_all`/`save`/`save_batch`（单事务）/`delete`/文件夹快照/`source_mtime` 快照 |
| `src/core/metadata.rs` | 元数据模型（`ItemMetadata`：paths + url/tags/star/annotation + 宽高 + `palette`/`palette_version`）。解析用 toml crate（宽容缺省）；**序列化手写**以精确控制输出格式（标量在前、`palette_version` 与标量同列于 `[[paths]]` 之前——数组表后的裸键会被解析进该表、缺省字段省略、字符串转义）；`is_valid_hash_file_name` 只认 64 位小写 hex（同步冲突副本自动忽略） |
| `src/core/metadata_store.rs` | 元数据存取：内存权威副本 + path→hash 反查表。构造即注水：缓存已注水走 `IndexDb.load_all` 快路径，失败或未注水回退 **TOML 全量解析**（顺带 `db.hydrate` 建缓存，每 1000 文件经 StartupState 报一帧进度）；写入铁律：**先 TOML 原子写（tmp+rename）成功，再刷内存副本与 SQLite**（`save`），中途崩溃朝 TOML 收敛；批量路径 `save_toml` 逐条落盘 + `apply_batch` 内存/SQLite 单事务统一应用（调色板批量回写用）；对账入口 `apply_external_toml`（只进不出，解析失败跳过不清空）与 `clear_external`（TOML 消失清空素材参数）；`hashes_with_missing_palette` 是派生缓存自愈的判定依据 |
| `src/core/item.rs` | 索引中的 item：`ItemLocation`（一个文件位置，回收站位置以 `.hawk/trash/` 开头）+ `Item`（位置列表 + 元数据查询副本 + 宽高 + 调色板，每项同时保存 RGB 与**预算 Lab** 检索坐标）；`sync_from` 为元数据 → 索引的单向同步（只允许流水线调用，palette 为 None 时保持索引现状）；`to_dto(trash_view)` 锁内投影——回收站视图的 paths 展示原库内路径（恢复目标）、folders 派生去重；`ItemQuery`/`ItemDto`/`ItemSkeletonDto` |
| `src/core/index.rs` | 内存索引：hash→item 与位置路径→hash 双字典，一把 `Mutex`。读取纪律见总览规则 3。`query` 实现 item/list 全部过滤（AND 语义）+ 排序 + 分页：**排序在轻量键上进行**（name 预计算小写键，避免比较器内反复分配）、主键同值按 id 字典序打破平局、desc 反转整个比较结果、`total_size` 为过滤后全量字节合计，**DTO 只投影分页窗口**（大库下不为全部命中项克隆完整 DTO）；`query_skeleton` 同过滤同排序投影 id/width/height/star（两次独立查询次序逐位一致，前端按 offset 取窗口依赖这一点）；颜色检索：查询色一次转 Lab，逐 item 检查预算 Lab 的 ΔE² ≤ 25²（免开方）；`folder_counts`（含祖先目录，同 item 同目录只计一次）/`category_counts`/`tags_with_counts` 供侧栏与 folder/list |
| `src/core/pipeline.rs` | **核心**：索引流水线，详见下节 |
| `src/core/content_hash.rs` | BLAKE3 流式哈希（1MB 缓冲），即 item id，元数据/缩略图命名依据 |
| `src/core/events.rs` | SSE 事件总线：tokio broadcast（容量 1024），订阅者消费跟不上（lagged）即由端点断开其订阅；`TaskProgress` 快照结构（task.progress 事件与 app/status 共用）；`folder.changed` 负载构造 |
| `src/core/fs_util.rs` | 文件操作辅助：建父目录、回收站冲突追加 ` (n)` 后缀（恢复按实际名称放回）、名称合法性校验（禁分隔符/`.`/`..`/`.hawk`） |
| `src/core/thumbnail.rs` | 图像服务：`identify`（只解码头取尺寸）、`detect_extension_bytes`（guess_format → 扩展名映射，item/add 类型推断用）、`generate`（Max 等比缩小**不放大**、Lanczos3、有损 WebP q80、缺失尺寸才生成/force 强制重建、按内容寻址存库外缓存目录）、`delete`。解码失败记 warn 并返回 false（读取端重试自愈）。`is_browser_renderable`：原图扩展名是否可被浏览器直接渲染（jpg/png/gif/webp/bmp），决定读取端未命中时能否回源原图（tiff 等必须走缩略图转换） |
| `src/core/color.rs` | 调色板提炼：降采样 64px（Triangle 滤波）→ **median-cut** 量化 ≤10 色 → 像素占比（alpha<128 不参与，0–100 保留 1 位小数，四舍五入同 v1 口径）；按占比降序、并列按 RGB 升序保证确定性；`PALETTE_VERSION=2`（C# 版 Wu=v1，旧结果视为未提炼由后台重提炼） |
| `src/core/color_math.rs` | 颜色纯函数：hex 解析/格式化、sRGB→CIELAB（D65）、CIE76 ΔE 的**平方**（与阈值平方比较免开方） |
| `src/core/thumbnail_worker.rs` | 缩略图/调色板后台 worker：**无界** mpsc 队列（任务 ~100B；C# 版「队列满静默丢弃」曾丢 20%+ 派生缓存）+ `CPU/2`（封顶 12）个专用 OS 线程（纯 CPU 任务靠 OS 调度让出 API）。任务两种（in-flight 去重 key 分命名空间）：`enqueue_thumbs`（**读取端未命中派发**，生成缺失尺寸）与 `enqueue_palette`（**入库/对账派发**，仅提炼调色板——缩略图是惰性缓存，不在入库时批量生成；颜色搜索依赖全量 palette，必须即时）。`process_job`：缩略图任务生成缺失尺寸；需调色板时优先**从最小尺寸的已有缩略图提炼**（解码代价小），无已有缩略图时直接解码原图 → 经回调回流水线（PaletteJob，单写者写入）；生成完成后取最新 DTO 补发 `item.updated`（前端 404 占位据此重建）；积压经 task.progress 500ms 节流可见 |
| `src/core/taxonomy.rs` | 分类/标签维度：`normalize_category_name`（扁平，无层级）；注册表骨架（固定 schema `key = [字符串数组]`，原子写、排序去重、重命名合并语义）→ `CategoryRegistry`/`TagRegistry`；`ItemEvents`：SSE 事件名常量与发布辅助（`publish_changed`/`publish_location_loss`/`publish_transition`，位置归零 → removed、只剩回收站 → trashed、首个库内位置回归 → restored）；`TaxonomyMigrator`：元数据写应用与全库级联迁移，**只被消费循环调用** |
| `src/core/view_prefs.rs` | 视图偏好注册表（`.hawk/view.toml`，参与同步）：扁平 map（scope 键 `folder:<路径>`/`category:<名>`/`tag:<名>`），不理解继承语义（前端沿父链解析）；scope/排序值白名单校验；`rename_prefix`/`delete_prefix` 跟随目录移动/删除；外部修改（含网盘同步落地）经 watcher 触发 `reload` |
| `src/core/scanner.rs` | 目录遍历（只读目录项，不读文件内容）：`walk_directory`（跳过 `.hawk` 内部、只深入 trash 子树、库内应用 ignore）；`walk_directory_stats`（产出 目录 → (mtime, 直接子项数) 供增量扫描快照对比，枚举失败置 `walk_incomplete`——调用方据此跳过消失对账防误删）；`walk_files_in_directory`（增量深入时只枚举直接文件） |
| `src/core/watcher.rs` | 文件监听（notify 封装）：原生粒度事件折叠为 FileSystemWatcher 语义（Create → upsert；Data/Metadata 修改 → 文件 upsert；Remove → Deleted；目录 → FolderCreated）；rename 的 From/To **配对带 300ms 超时兜底**（滞留 From 按删除处理，150ms ticker 周期 flush + 事件到达顺带 flush；并发多 rename 错配由幂等流水线 + 超时兜底自愈）；config.toml / categories.toml / tags.toml / view.toml 单独上报驱动热更；缓冲 Overflow → 全量扫描兜底；`.hawk` 内部（回收站除外）不产生索引事件 |
| `src/core/startup.rs` | 启动状态：进度快照（phase/processed/total）、就绪标志、失败原因。`/health`、就绪网关与 `app/startup` 端点的共同数据源 |

### IndexPipeline 详解

所有索引变更收敛为单消费者循环（专用线程 `hawk-index-pipeline`），Job 经有界 channel（4096）串行处理：

- **Job 类型**：`Upsert` / `Delete` / `Move` / `DirMove` / `Scan` / `ClearTrash` / `Metadata` / `BatchMetadata` / `MetadataSync` / `Palette` + `PaletteFlush`（调色板批量回写）/ `FolderHint` / `CategoryCreate|Update|Delete` / `TagCreate|Update|Delete` / `RegistryReload`
- **两类入口**：watcher 走 fire-and-forget，channel 满置 overflow 标记（消费者检查后入队**去重的 ScanJob** 兜底，不内联扫描，避免事件风暴期反复全库遍历）；API 与启动走携带 `oneshot` 回复通道的提交，等待处理完成后返回。消费循环对每个 Job 做 `catch_unwind`，panic 的任务被跳过且等待中的调用方收到错误（不会挂起）
- **入库（do_upsert）拆两步**：`prepare_upsert`（路径过滤 `.hawk`/ignore、stat、**哈希复用判定**——路径与 size/mtime 同元数据一致即复用不读内容、写入中文件防抖——mtime 距今不足 1s 延迟重试，同路径去重上限 120 次）→ 哈希（`known_hash` 直接采用 / 复用 / 计算）→ **复验 size/mtime**（哈希期间仍在写入则延迟重试，不以半截内容入库，慢速拷贝的哈希漂移由此根治）→ `apply_upsert`（串行应用）
- **`apply_upsert`**：哈希漂移时先取旧元数据用于继承，再按路径迁移（新 item 继承 tags 等素材参数，旧元数据无引用则删除元数据与缩略图）并发布位置丢失事件；元数据登记路径并回写最新 size/mtime；**宽高在入库时即持久化入 TOML**（C# 版仅内存更新，重启靠扫描重新识别）；索引同步后发 `item.added`/`item.updated`；`needs_palette_work`（调色板缺失/版本旧）才派发 worker（仅调色板任务）——缩略图为惰性缓存不在入库生成；已齐备文件（对账重放）派发 no-op 任务会把积压计数灌满失真
- **全库扫描（do_scan，三阶段 + 目录快照）**：阶段一 `walk_directory_stats` 与上轮 `folders` 快照（mtime + 直接子项数）对比，**只有 dirty 目录才深入文件级访问**（clean 目录不碰文件系统）；阶段二 dirty 目录枚举文件做复用判定/入库（复用项立即应用）；阶段三需要哈希的文件**并行计算**（`CPU-1` 封顶 24，留 1 核给 API；图像尺寸识别同属只读阶段一并并行；哈希后复验，仍在写入的延迟重试），索引/元数据应用仍串行（单写者不变）；**消失对账**——所在目录已不存在、或 dirty 目录深入后未见的位置判为消失（clean 目录快照一致则位置必然还在，不访问）；遍历不完整时本轮跳过消失对账与快照替换（防误删），最终一致由下轮对账保证；快照整体替换为本轮统计；完成后广播一次 `folder.changed`
- **元数据对账（do_metadata_sync，只进不出）**：`.hawk/metadata/` 的 TOML 是唯一权威源（参与网盘同步），本机缓存与内存副本经此跟随外部变更。按文件 mtime 与 `IndexDb.source_mtime` 比对，只有变化的文件才重解析；新增/变更 → `apply_external_toml` 载入后刷新索引副本、登记注册表、发 `item.updated`；TOML 消失 → `clear_external` 清空素材参数（item 与位置由扫描决定存续）；解析失败跳过且不清空状态，下轮重试。每 100 文件经 StartupState 报一帧进度（对账可能持续很久，启动屏靠它续命）。**派生缓存自愈**：`hashes_with_missing_palette` 纯内存扫描出的缺失项派发 worker 补齐（源文件已不在的由 worker 静默跳过，删除对账收敛位置）
- **调色板批量回写**：worker 提炼结果经 PaletteJob 进入暂存（同 hash 以最新为准），≥500 条立即冲刷、滞留 2s 由一次性定时任务入队 PaletteFlush 唤醒（队列安静期也能落盘）；冲刷 = 逐条 `save_toml`（铁律：TOML 先行）→ `apply_batch` 内存 + SQLite 单事务 → 逐 item 同步索引补发 `item.updated`——全库重提炼（v1→v2 迁移、缓存重建）时无 N 次单条事务、无事件洪峰
- **移动（do_move / do_dir_move）**：目标路径不可用（`.hawk` 内部/被 ignore）时整体转删除；`move_one` 只做索引 rekey 与元数据路径跟随（lib↔trash 去前缀后库内路径不变，元数据保持原路径 = 恢复目标），**不重算哈希**；目录移动批量 rekey 后 `prefs.rename_prefix` 让排序偏好跟随、`publish_transition` 广播事件、补扫新位置吸收监听遗漏的子文件事件
- **清空回收站（do_clear_trash）**：摘除全部回收站位置并清理元数据 paths；内容在库内无其他引用时删除元数据与缩略图；物理删除由 API 层在其后完成
- **进度上报**：`ScanReporter` 150ms 节流（阶段切换/总数变化强制发帧）写 StartupState（→ `/app/startup`）；`task.progress(index)` 500ms 节流 + 空闲转变化时补发清零帧

### api/item.rs 详解

- **list / skeleton / detail / count**：纯查询。list/skeleton 共用 `build_query`（同一转换路径保证两次查询次序逐位一致），走 `ItemIndex::query` / `query_skeleton`；detail 走 `get_dto` 锁内投影
- **add**：`path`/`url`/`img_base64` 三选一取内容（url 经 ureq 30s 超时下载到内存，扩展名从 URL 推断、推断不出按内容嗅探；base64 必须能被 image 识别否则 `UNSUPPORTED_FORMAT`）→ 目标已存在报 `FILE_EXISTS` → **写入前先算哈希**确定 `already_existed`（避免 watcher 竞态改变语义）→ 文件落库（path 导入保留原文件 mtime/atime）→ `submit_upsert` 携带已知哈希（流水线跳过重算，大文件免二次读盘）→ 附带的 tags/categories/annotation/website 经 `submit_metadata` 写入 → 响应取 `get_dto` 最新投影
- **update**：经 `find_location` 取位置快照 → 回收站中的文件禁止改名/移动 → `name` 分支做真实 rename 并 `submit_move`；`folder_path` 分支**按移动后的最新位置再移动**（改名+移动同请求时基于新文件名计算目标）→ tags/star(0–5 校验)/categories/annotation/url 走 `submit_metadata` → 响应 `get_dto` 投影
- **batch_update**：校验后先逐个移动主位置（同名冲突不整体失败，跳过该项记入 missing），再 `submit_batch_metadata` 一次提交（标签/分类并集追加、评分设置）；不存在的 id 由流水线记入 `missing_ids`；无任何更新字段返回 400
- **delete / restore**：delete 把文件移入 `.hawk/trash/`（保留目录结构，冲突加 ` (n)` 后缀）；restore 按回收站实际名称去掉前缀后的路径放回，被占用报 `FILE_EXISTS`；位置定位均走 `find_location` 快照
- **thumbnail**：尺寸必须在 `thumbnail_sizes` 白名单内。缩略图为**惰性缓存**（入库/对账不生成）：命中返回 webp；未命中且浏览器可渲染（jpg/png/gif/webp/bmp）→ **直接回源原图**（200，同时后台入队生成缓存）；不可渲染格式（tiff 等）后台生成、生成中 404（经 `item.updated` 重建）。响应 `Cache-Control: immutable`（id 是内容哈希，内容永不变）——注意未命中回源的原图响应同样 immutable，客户端可能长期持有原图字节而非升级 webp（视觉无损，接受）
- **file**：主位置（优先库内）原图二进制，Content-Type 按扩展名 `mime_guess`；同样 immutable、放行 `?token=`
- **refresh_thumbnail**：取可读主位置强制重建全部尺寸
- **replace**（`item/replace`）：客户端编辑（旋转/裁切）后的新内容提交存储层。内容必须可识别且**格式与文件扩展名一致**（扩展名与内容错位会破坏类型推断与预览）；哈希相同则幂等直接返回当前投影；写回保留原 mtime（修正性编辑不改变素材的时序位置）；`submit_upsert` 触发 id 漂移闭环（元数据继承迁移/事件/缩略图重建）

## 关键流程串联

### 1. 启动建索引（先监听、后索引）

```text
绑定 127.0.0.1（+ 可选 LAN）→ axum::serve 拉起（startup 端点已可答 starting）
    └► MetadataStore 构造即注水：SQLite 缓存（快路径）→ TOML 全量解析（回退，每 1000 文件一帧进度）
    └► hydrate_index：元数据副本 → 内存索引（秒级）
    └► pipeline.start()：消费线程 + ThumbnailWorker + 周期对账定时 + 首轮 MetadataSync（先于扫描）
    └► watcher 接线（事件先入队，与对账扫描天然去重）
    └► startup.mark_ready() → /health 200、API 网关放行
    └► 后台 run_scan(false)：目录快照对比 → dirty 目录深入 → 并行哈希 → 串行应用
         停机期间的增删改由此收敛（就绪到扫描完成之间有秒级~分钟级窗口，运行期变更由 watcher 实时覆盖）
```

握手无 stdout 私有协议：端口由 Electron 预选传入，进度/就绪经 `/api/v1/app/startup` 轮询；
初始索引完成前 `/api/*` 返回 503 `NOT_READY`（`app/startup` 除外）。如对索引状态存疑，
可手动调用 `POST /api/v1/library/reindex` 全量重建。

### 2. 外部文件变更（用户在文件管理器里操作）

```text
LibraryWatcher（notify 事件折叠 + rename 配对）→ FileUpsert/Deleted/Moved
  → pipeline 入队 → 防抖（写入中文件延迟重试，上限 120 次）→ 哈希/迁移/索引更新
  → MetadataStore 落盘 → EventBus → SSE → 前端增量刷新
```

缓冲溢出或事件丢失：置 overflow 标记，消费循环入队去重的 ScanJob 兜底（扫描把所有待处理文件一次收敛）。

### 3. 网盘同步落地（另一台设备改的 TOML 出现在 .hawk/metadata/）

```text
周期对账（默认 60s，启动时先跑一轮）→ mtime 与 source_mtime 比对
  → 新增/变更：apply_external_toml 载入 → 刷索引副本 → 登记注册表 → 发 item.updated
  → 消失：clear_external 清空素材参数
  → palette 缺失/版本旧：派发仅调色板任务补齐（缩略图不在对账中批量生成，由读取端惰性触发）
```

### 4. item/add

```text
api: 取内容（复制/下载/解码）→ 预计算哈希 → 文件写入库内
   → submit_upsert（携带已知哈希，流水线跳过重算）→ submit_metadata（素材参数）
   → 返回 item + already_existed
watcher 随后上报的 Created 事件幂等吸收
```

### 5. 回收站三段操作

```text
item/delete:   rename 到 .hawk/trash/<原路径>（冲突加后缀）→ submit_move
               元数据不动（paths 仍是原路径 = 恢复目标）
item/restore:  rename 回原路径（占用 → FILE_EXISTS）→ submit_move
trash/clear:   submit_clear_trash 清位置、清元数据 paths；库内无引用时删元数据与缩略图
               → 随后物理删除 .hawk/trash/ 内容（顺序不能颠倒）
```

## 测试

| 层 | 位置 | 说明 |
| ---- | ---- | ---- |
| 单元测试 | `cargo test`（各文件内联 `#[cfg(test)]`） | 纯函数与纯数据结构：颜色数学（hex/Lab/ΔE 已知向量）、median-cut 提炼（纯色→1 色 ~100%、双色各半、全透明→空、占比合计）、TOML 解析/序列化往返与转义、ignore 匹配、路径换算与回收站前缀、BLAKE3 标准向量、视图偏好校验 |
| 端到端契约测试 | `tools/smoke.sh`（81 项断言） | 临时素材库 + curl 覆盖 HTTP API 全流程（鉴权、索引、过滤、颜色检索、缩略图、去重、文件夹、监听防抖、SSE、batch_update、回收站、重启哈希复用）。语言无关的契约测试，需先 `cargo build --release` |

测试策略：契约级测试（HTTP/存储格式）优先于内部单元测试——行为对齐以 OpenAPI schema + `.hawk/` 存储格式 + SSE 事件契约为准。

## 工程说明

| 文件 | 说明 |
| ---- | ---- |
| `Cargo.toml` | 二进制名保持 `hawk-server`（app 集成路径一致）；release 配置 `lto = true` + `codegen-units = 1` + `panic = "abort"` + `strip = true`（单文件 ~9MB） |
| `openapi.json` | 契约 schema（`include_str!` 固化），变更后直接编辑该文件，见 server-rust.md「OpenAPI schema」节 |
| `tools/bench-*.py` | 性能压测（启动/入库/图像管线/大库全链路），输出带 git SHA 的 RESULT 行，基线见 server-rust.md |

## 排查指引

- 索引不一致 → `POST /api/v1/library/reindex` 全量重建；watcher 缓冲溢出会自动触发同样的兜底扫描；`POST /api/v1/library/rescan` 忽略目录快照强制遍历（不读文件内容）
- 缩略图 404 → 仅应出现在不可渲染格式（tiff 等）生成期间，生成完成后经 `item.updated` 自动重建；持续 404 调 `item/refresh_thumbnail`；解码失败的素材每次查看都会重试（入队以 identify 为闸）
- 派生缓存损坏 → 删除库外缓存目录（`<系统缓存>/hawk/cache/<库标识>/`）重启自动重建（TOML 是唯一权威源）
- 行为对齐以 OpenAPI schema（`/openapi/v1.json`）为准，`.hawk/` 存储格式与 SSE 事件契约（事件名、负载形状）为持久化契约

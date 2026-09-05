# hawk-daemon 代码导读

面向新加入者的代码地图：`hawk-daemon/src/` 每个文件做什么、与谁协作，以及几条关键流程如何把代码串起来。
设计背景见 [server-rust.md](server-rust.md)（技术选型与状态）、[storage.md](storage.md)（存储格式）、
[server-rest-api-v1.md](server-rest-api-v1.md)（接口契约）、[architecture.md](../architecture.md)（进程模型）。

## 总览

hawk-daemon 分两层，依赖单向：`api/` → `core/`。HTTP 请求不直接碰可变索引，
写路径一律把变更作为 Job 提交给索引流水线并等待完成。

```text
HTTP 请求 ──► api/（端点、信封、鉴权中间件）──► 读：ItemIndex 锁内投影（DTO / 骨架）
                                        └► 写：真实文件操作 → IndexPipeline 提交任务并等待（60s 超时）
文件系统变化 ──► LibraryWatcher ──► IndexPipeline（Job 入队，有界 channel 4096）
                                        │（专用消费线程，单写者）
                                        ▼
                              IndexPipeline（消费循环）
                                哈希 → 元数据迁移 → 更新 ItemIndex
                                → 写 MetadataStore → 派发缩略图 → EventBus 发 SSE
                                        ▲
                              扫描 runner 线程（walk/并行哈希）
                                与 worker 的结果均以 Job 回流队列，
                                消费循环穿插应用（长扫描不独占消费线程）
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
| `src/main.rs` | 进程入口（薄壳，~30 行）：`Cli::parse_args` → `--dump-openapi` 分流（打印 schema 后退出）→ tracing 初始化 → `bootstrap::run`。组装与启动编排全部在 bootstrap |
| `src/bootstrap.rs` | 组件图组装与启动编排（组合根）：`build_state` 直接构造 `api::AppState`（单一共享状态对象，按依赖分层分段：存储底座 → 索引流水线 → 分类与视图偏好 → 缩略图 → LAN；db/store/scanner/migrator 是构造中间件不进组件图；worker.attach 回流接线须在 start 前）；`run` 按序执行：`resolve_port` 试绑 27371（占用回退动态分配）→ `build_router` → **先监听**（环回 `axum::serve` 优雅退出；LAN 由 supervisor 常驻任务管理，首轮回合即按配置绑定）→ `pipeline.start()`（注水/消费线程/worker/周期对账）→ 接线 watcher（ConfigChanged 先 `config.reload()` 再按差异分发：ignore 变化 → 强制重扫；[web] 变化 → LAN 热重绑）→ `startup.mark_ready()` → 后台全库对账扫描 |
| `src/settings.rs` | 启动设置（解析层/配置层分离）：`Cli` 用 clap derive 声明全部 CLI（`--library`/`--port`/`--web-dist`/`--cache-parent`/`--dump-openapi`，每项支持同名 `HAWK_*` env 回退、CLI 优先，未知参数/非法值报错 exit 2，`--dump-openapi` 豁免 library 必填）；`Settings::from_cli` 承接业务校验（库目录为空/不存在 exit 2）与 env-only 参数——token 只走 `HAWK_TOKEN`（避免出现在进程列表，未传入时生成随机值并打印 stdout），`HAWK_RESCAN_INTERVAL` 对账间隔（默认 60s，0 关闭） |

### api/ —— HTTP 层

| 文件 | 职责 |
| ---- | ---- |
| `src/api/lan.rs` | LAN 监听 supervisor（期望态收敛）：持有当前 0.0.0.0 监听，被 watcher 的 ConfigChanged 唤醒后对比期望态（`[web]` enabled 且 token 非空 → port）与实际，差异时优雅关停旧监听（3s 排空超时强杀）重绑新端口；仅 token 变化为 no-op（token 每请求经 `current().web` 校验天然热）；绑定失败降级为状态错误（app/info 暴露，设置面板轮询），不崩进程 |
| `src/api/mod.rs` | `AppState` 共享状态与 `build_router`；中间件链 cors → auth → ready_gate（axum 中后注册的 layer 在外层，请求依次经过）；auth 双 token：admin（进程 token，全权限）与 viewer（局域网 token，`AccessLevel::Viewer { writable }` 携带 per-token 写能力：未开启写则只读，开启且未拆分时 token 读写兼具，拆分时 token 只读、write_token 可写——每请求经 current().web 判定，热生效），`/api/v1/events`、`item/thumbnail`、`item/file` 三个无法设请求头的 GET 端点放行 `?token=`；ready_gate 未就绪时 `/api/*` 一律 503 `NOT_READY`，仅放行 `app/startup`；全局 DefaultBodyLimit 256MB（upload 整文件与 replace 的 base64 超出 axum 默认 2MB） |
| `src/api/envelope.rs` | 统一成功/错误信封、错误码常量、`ApiError` → 响应的 `IntoResponse`；`JsonBody` 提取器把 JSON 解析失败统一转为 `INVALID_PARAM` |
| `src/api/app.rs` | `health`（就绪前 503，无需 token）、`startup`（starting 带进度 / ready / error）、`status`（缩略图与索引积压快照，轮询型客户端用）、`info`（版本/平台/可执行路径/access 级别）、`token`（token 发现：Host 限定环回地址防 DNS rebinding，响应不带 CORS 头） |
| `src/api/library.rs` | `library/info`（显示名取 config 的 name，缺省目录名）、`reindex`（全量重哈希扫描，异步立即返回）、`rescan`（忽略目录快照强制遍历，仍按 size/mtime 复用哈希） |
| `src/api/folder.rs` | folder 五端点。list 为**目录结构缓存 + 实时计数合并**（`FolderTreeCache`：递归建树的文件系统遍历结果缓存，count 每次从索引 `folder_counts` 覆写；失效与 folder.changed 同线——API 写端点内同步失效，外部改动经总线订阅失效，ignore 配置变更亦失效）；create/update/delete/restore 先做校验（名称合法、父目录存在、`FILE_EXISTS`、禁止移入自身子目录）再做真实目录操作，变更经 `submit_dir_move` 同步索引（DirMoveJob 内广播 folder.changed，端点不重复通知） |
| `src/api/taxonomy.rs` | category/tag 八端点。list = 注册表 ∪ 全部 item 赋值并集（count 不含回收站）；名称校验在端点层，写全部 `submit_*` 给流水线执行 |
| `src/api/view.rs` | view 三端点（preferences / preference PUT / DELETE）。偏好与索引/元数据无耦合，注册表自带锁，端点直接读写，**不经过索引流水线** |
| `src/api/trash.rs` | `trash/clear`。顺序铁律：先 `submit_clear_trash` 清索引位置、元数据与缓存（缩略图/调色板），再物理删除 `.hawk/trash/` 内容——先物理删除会让 watcher 的 Deleted 事件抢先摘除位置，导致元数据与缓存泄漏 |
| `src/api/events.rs` | SSE 订阅端点：`broadcast::Receiver` 转 `event:`/`data:` 帧；lagged（消费跟不上）或总线关闭即结束流，客户端重连后须以 `item/skeleton` + `folder/list` 全量对齐。同文件定义 `SseEvents` 事件载荷注册表（键=事件名，与 `ItemEvents` 常量一一对应，契约测试双向比对），经 `attach_extra_schemas` 注册进 OpenAPI components |
| `src/api/openapi.rs` | OpenAPI 文档装配（`build_openapi_json`：OpenApiRouter 收集 + SSE 事件载荷等无端点引用 schema 的手工注册 + default 关键字剥除）与 `/openapi/v1.json` 服务（OnceLock 缓存）；schema 由代码生成并固化入库，同步性由契约测试校验 |
| `src/api/web_dist.rs` | 局域网 web 查看的静态托管（fallback 挂载；`--web-dist` 传入时启用）：SPA 回退 `index.html`，`/assets/` 内容哈希资源 immutable 长缓存，其余 no-cache（防手机浏览器启发式缓存旧 HTML）；favicon 经 vite publicDir=build/ 落在 dist 根（icon.png，index.html 内 link rel=icon 引用），由本服务一并返回 |
| `src/api/item/` | item 十四端点，逻辑最重，按子域拆分（`mod.rs` 聚合路由与公共辅助；`query.rs` list/skeleton/detail/count；`add.rs` 路径导入与 URL 下载；`upload.rs` multipart 上传；`update.rs` update/batch_update；`delete.rs` 回收站进出；`file.rs` thumbnail/file/refresh_thumbnail；`replace.rs` 内容替换），见下节 |
| `src/api/contract_tests.rs`（`#[cfg(test)]`） | OpenAPI 契约校验：schema 声明的端点全量归类（新增端点未归类即失败）、空库成功路径响应体经 jsonschema 校验（$ref 经 components 提升解析）、写端点校验路由存在（区分业务错误信封与 fallback 空 404） |

### core/ —— 与 HTTP 无关的领域核心

| 文件 | 职责 |
| ---- | ---- |
| `src/core/paths.rs` | 路径规则的唯一权威：`.hawk/` 布局与 `ensure_layout`（含给 `.hawk/.gitignore` 补 `trash/` 排除项）、库内相对路径 ↔ 绝对路径互转（`..`/越界/绝对路径一律拒绝）、`is_internal`（`.hawk` 内部，回收站除外）、回收站前缀换算、`is_valid_library_path`（API 入参守卫）、库外缓存目录命名（`<库文件夹名>_<根路径 SHA-256 前16位>`，同名库靠哈希区分）、纯路径工具（`full_path` 文本归约、`dir_of`/`name_of`/`ext_of`、`unix_ms`/`file_mtime_ms`） |
| `src/core/config.rs` | `.hawk/config.toml` 快照（RwLock）与热更 `reload`（返回 `ConfigChange` 差异：ignore/web 是否变化）；库首次打开生成带注释的默认配置；ignore 匹配器：无 `/` 的模式展开为 `**/p` + `**/p/**`（任意深度同名），分段 glob（`**` 跨目录、段内 `*`/`?`），大小写不敏感 |
| `src/core/index_db.rs` | 元数据 SQLite 派生缓存（库外 `index.db`）：schema v1（`items`/`paths`/`tags`/`categories`/`folders` 五表 + `meta`，版本不符整库重建），`journal_mode=DELETE` 不产生 -wal/-shm，`busy_timeout` 5s；`hydrated` 注水标记（false 时内容不可信，必须由 TOML 全量重建）；打开失败/写失败/读失败一律 **poison 熔断**，退化为纯 TOML 模式——缓存故障绝不影响权威数据；`load_all`/`save`/`save_batch`（单事务）/`delete`/文件夹快照/`source_mtime` 快照 |
| `src/core/metadata.rs` | 元数据模型（`ItemMetadata`：paths + url/tags/star/annotation + 宽高 + `palette`）。解析用 toml crate（宽容缺省）；**序列化手写**以精确控制输出格式（标量在前、`[[paths]]`/`[[palette]]` 数组表在后——数组表后的裸键会被解析进该表、缺省字段省略、字符串转义）；`is_valid_hash_file_name` 只认 64 位小写 hex（同步冲突副本自动忽略） |
| `src/core/metadata_store.rs` | 元数据存取：内存权威副本 + path→hash 反查表。构造即注水：缓存已注水走 `IndexDb.load_all` 快路径，失败或未注水回退 **TOML 全量解析**（顺带 `db.hydrate` 建缓存，每 1000 文件经 StartupState 报一帧进度）；写入铁律：**先 TOML 原子写（tmp+rename）成功，再刷内存副本**（`save`）；SQLite 缓存写进**待冲刷缓冲**（≥256 条或滞留 200ms 单事务冲刷，消费循环每任务后检查、扫描收尾强制冲刷）——缓存可重建，崩溃后由启动期对账按 mtime 从 TOML 补齐；直写缓存的路径（delete/apply_batch/apply_external_toml/clear_external/replace_folder_snapshots）先冲刷缓冲或移除同 hash 待写项，避免旧值覆盖新值，中途崩溃朝 TOML 收敛；批量路径 `save_toml` 逐条落盘 + `apply_batch` 内存/SQLite 单事务统一应用（调色板批量回写用）；对账入口 `apply_external_toml`（只进不出，解析失败跳过不清空）与 `clear_external`（TOML 消失清空素材参数）；`hashes_with_missing_palette` 是派生缓存自愈的判定依据 |
| `src/core/item.rs` | 索引中的 item：`ItemLocation`（一个文件位置，回收站位置以 `.hawk/trash/` 开头）+ `Item`（位置列表 + 元数据查询副本 + 宽高 + 调色板，每项同时保存 RGB 与**预算 Lab** 检索坐标）；`sync_from` 为元数据 → 索引的单向同步（只允许流水线调用，palette 为 None 时保持索引现状）；`to_dto(trash_view)` 锁内投影——回收站视图的 paths 展示原库内路径（恢复目标）、folders 派生去重；`ItemQuery`/`ItemDto`/`ItemSkeletonDto` |
| `src/core/index.rs` | 内存索引：hash→item 与位置路径→hash 双字典，一把 `Mutex`。读取纪律见总览规则 3。`query` 实现 item/list 全部过滤（AND 语义）+ 排序 + 分页：**排序在轻量键上进行**（name 预计算小写键，避免比较器内反复分配）、主键同值按 id 字典序打破平局、desc 反转整个比较结果、`total_size` 为过滤后全量字节合计，**DTO 只投影分页窗口**（大库下不为全部命中项克隆完整 DTO）；`query_skeleton` 同过滤同排序投影 id/width/height/star（两次独立查询次序逐位一致，前端按 offset 取窗口依赖这一点）；颜色检索：查询色一次转 Lab，逐 item 检查预算 Lab 的 ΔE² ≤ 25²（免开方）；`folder_counts`（含祖先目录，同 item 同目录只计一次）/`category_counts`/`tags_with_counts` 供侧栏与 folder/list |
| `src/core/pipeline/` | **核心**：索引流水线模块（单写者 actor），按职责拆分，详见下节：`mod.rs`（Job 枚举/提交表面/消费循环/超时）、`ctx.rs`（上下文 + JobSender 外部回流通道 + 进度快照）、`upsert.rs`（入库生命周期）、`fs_ops.rs`（删除/移动/回收站）、`scan.rs`（扫描 runner + 会话簿记）、`reconcile.rs`（元数据对账）、`derived.rs`（调色板/宽高回写） |
| `src/core/content_hash.rs` | BLAKE3 流式哈希（1MB 缓冲），即 item id，元数据/缩略图命名依据 |
| `src/core/events.rs` | SSE 事件总线：tokio broadcast（容量 1024），订阅者消费跟不上（lagged）即由端点断开其订阅；`TaskProgress` 快照结构（task.progress 事件与 app/status 共用）；`folder.changed` 负载构造 |
| `src/core/fs_util.rs` | 文件操作辅助：建父目录、回收站冲突追加 ` (n)` 后缀（恢复按实际名称放回）、名称合法性校验（禁分隔符/`.`/`..`/`.hawk`） |
| `src/core/thumbnail.rs` | 图像服务：`identify`（只解码头取尺寸）、`detect_extension_bytes`（guess_format → 扩展名映射，item/add 类型推断用）、`generate`（唯一尺寸 1024，Max 等比缩小**不放大**、Lanczos3、有损 WebP q80、已存在跳过/force 强制重建、按内容寻址存库外缓存目录）、`delete`。解码失败记 warn 并返回 false（读取端重试自愈）。`is_browser_renderable`：原图扩展名是否可被浏览器直接渲染（jpg/png/gif/webp/bmp），决定读取端未命中时能否回源原图（tiff 等必须走缩略图转换） |
| `src/core/color.rs` | 调色板提炼：降采样 64px（Triangle 滤波）→ **median-cut** 量化 ≤10 色 → 像素占比（alpha<128 不参与，0–100 保留 1 位小数）；按占比降序、并列按 RGB 升序保证确定性 |
| `src/core/color_math.rs` | 颜色纯函数：hex 解析/格式化、sRGB→CIELAB（D65）、CIE76 ΔE 的**平方**（与阈值平方比较免开方） |
| `src/core/thumbnail_worker.rs` | 缩略图/调色板后台 worker：**无界** mpsc 队列（任务 ~100B，不丢弃）+ `CPU/2`（封顶 12）个专用 OS 线程（纯 CPU 任务靠 OS 调度让出 API）。任务三种（in-flight 去重 key 分命名空间，PaletteOnly 与其余互不挤占）：`enqueue_palette`（**入库/对账/读取端宽高自愈派发**，提炼调色板 + 补缺失宽高——缩略图是惰性缓存，不在入库时批量生成；颜色搜索依赖全量 palette，必须即时）、`enqueue_thumbs`（**读取端未命中与 `library/refresh_cache` 范围刷新派发**，补缺失宽高 + 生成缺失缩略图 + 按需调色板，不重建已有文件）、`enqueue_force_rebuild`（**单 item 手动刷新派发**，强制重建缩略图）。`process_job` 共有前置 `ensure_dim`：宽高为 0 时 `identify`（只解头部）经 JobSender 回流队列回写（FixDim，单写者）；需调色板时优先**从已有缩略图提炼**（解码代价小），无已有缩略图时直接解码原图 → 经 JobSender 回流队列（Palette）；对索引/元数据只做只读访问（`get_dto`/`dim_is_zero`/`try_get`），生成或宽高修复完成后取最新 DTO 补发 `item.updated`（前端 404 占位与 0 × 0 卡片据此重建）；积压经 task.progress 500ms 节流可见 |
| `src/core/taxonomy.rs` | 分类/标签维度：`normalize_category_name`（扁平，无层级）；注册表骨架（固定 schema `key = [字符串数组]`，读写公共件在 `registry_file.rs`，重命名合并语义）→ `CategoryRegistry`/`TagRegistry`；`ItemEvents`：SSE 事件名常量与发布辅助（`publish_changed`/`publish_location_loss`/`publish_transition`，位置归零 → removed、只剩回收站 → trashed、首个库内位置回归 → restored）；`TaxonomyMigrator`：元数据写应用与全库级联迁移，**只被消费循环调用** |
| `src/core/view_prefs.rs` | 视图偏好注册表（`.hawk/view.toml`，参与同步）：扁平 map（scope 键 `folder:<路径>`/`category:<名>`/`tag:<名>`），不理解继承语义（前端沿父链解析）；scope/排序值白名单校验；`rename_prefix`/`delete_prefix` 跟随目录移动/删除；外部修改（含网盘同步落地）经 watcher 触发 `reload` |
| `src/core/global_filter.rs` | 全局列表隐藏项注册表（`.hawk/global_filter.toml`，参与同步）：folders/categories/tags 三个列表；端点直接读写（同 view_prefs，不过流水线），级联跟随（文件夹移动/删除、分类/标签改名删除）由流水线对应 Job 调用；变更广播 `global_filter.changed` |
| `src/core/registry_file.rs` | 注册表文件持久化公共件：原子写（临时文件 + rename）、字符串列表键解析（trim/去空/去重/小写排序）与格式化；taxonomy/global_filter/view_prefs 共用 |
| `src/core/scanner.rs` | 目录遍历（只读目录项，不读文件内容）：`walk_directory`（跳过 `.hawk` 内部、只深入 trash 子树、库内应用 ignore）；`walk_directory_stats`（产出 目录 → (mtime, 直接子项数) 供增量扫描快照对比，枚举失败置 `walk_incomplete`——调用方据此跳过消失对账防误删）；`walk_files_in_directory`（增量深入时只枚举直接文件） |
| `src/core/watcher.rs` | 文件监听（notify 封装）：原生粒度事件折叠为 FileSystemWatcher 语义（Create → upsert；Data/Metadata 修改 → 文件 upsert；Remove → Deleted；目录 → FolderCreated）；rename 的 From/To **配对带 300ms 超时兜底**（滞留 From 按删除处理，150ms ticker 周期 flush + 事件到达顺带 flush；并发多 rename 错配由幂等流水线 + 超时兜底自愈）；config.toml / categories.toml / tags.toml / view.toml 单独上报驱动热更；缓冲 Overflow → 全量扫描兜底；`.hawk` 内部（回收站除外）不产生索引事件 |
| `src/core/startup.rs` | 启动状态：进度快照（phase/processed/total）、就绪标志、失败原因。`/health`、就绪网关与 `app/startup` 端点的共同数据源 |

### IndexPipeline 详解

所有索引变更收敛为单消费者循环（专用线程 `hawk-index-pipeline`），Job 经有界 channel（4096）串行处理：

- **Job 类型**：`Upsert` / `Delete` / `Move` / `DirMove` / `ScanStart`+`ScanFile`+`ScanEnd`（扫描三段，见下）/ `ClearTrash` / `Metadata` / `BatchMetadata` / `MetadataSync` / `Palette` + `PaletteFlush`（调色板批量回写）/ `FixDim`（宽高补全）/ `FolderHint` / `CategoryCreate|Update|Delete` / `TagCreate|Update|Delete` / `RegistryReload`
- **两类入口**：watcher/worker 走 fire-and-forget（经 `JobSender`），channel 满置 overflow 标记（消费者检查后入队**去重的扫描**兜底，不内联扫描，避免事件风暴期反复全库遍历）；API 与启动走携带 `oneshot` 回复通道的提交，统一 60s 超时等待（`run_scan` 例外——等整轮扫描完成，不设超时），消费循环停止/任务 panic 时调用方立即收到错误而非挂起。消费循环对每个 Job 做 `catch_unwind`，panic 的任务被跳过
- **入库（do_upsert）拆两步**：`prepare_upsert`（纯判定、无副作用——路径过滤 `.hawk`/ignore、stat、**哈希复用判定**（路径与 size/mtime 同元数据一致即复用不读内容）、写入中文件防抖（mtime 距今不足 1s 延迟重试，同路径去重上限 120 次）；需要删除路径时返回 Remove 由调用方执行，消费循环内联、扫描 runner 经队列回流）→ 哈希（`known_hash` 直接采用 / 复用 / 计算）→ **复验 size/mtime**（哈希期间仍在写入则延迟重试，不以半截内容入库，慢速拷贝的哈希漂移由此根治）→ `apply_upsert`（串行应用；创建 item 与登记位置在同一锁内完成，零位置 item 不对并发查询可见）
- **`apply_upsert`**：哈希漂移时先取旧元数据用于继承，再按路径迁移（新 item 继承 tags 等素材参数，旧元数据无引用则删除元数据与缩略图）并发布位置丢失事件；元数据登记路径并回写最新 size/mtime；扫描路径携带的调色板（并行阶段单次解码提炼）随首版 TOML 一并持久化；**宽高在入库时即持久化入 TOML**；索引同步后发事件——扫描路径合并进 `items.added` 批量事件（300ms 窗口/2000 条上限/扫描结束兜底冲刷），单条路径发即时 `item.added`/`item.updated`；`needs_palette_work`（调色板缺失/版本旧）才派发 worker（仅调色板任务）兜底增量与解码失败自愈——扫描导入的缩略图已在并行阶段生成，读取端惰性兜底仍在
- **全库扫描（三段任务 + 目录快照，消费线程不被长扫描独占）**：`ScanStart` 只建会话并 spawn **runner 线程**（`hawk-scan`）——阶段一 `walk_directory_stats` 与上轮 `folders` 快照（mtime + 直接子项数）对比，**只有 dirty 目录才深入文件级访问**（clean 目录不碰文件系统）；阶段二 dirty 目录枚举**直接文件**（目录项跳过；同名「文件→目录」替换由消失对账收敛）做复用判定，复用项直接回流；阶段三需哈希文件进**并行导入通道**（物理核估计（逻辑核/2，封顶 16）：解码/编码 SIMD 密集，SMT 无增益，SMT 兄弟线程留给消费循环与 API）：哈希（后复验，仍在写入的延迟重试）→ 单次解码同出 调色板/缩略图/宽高（派生齐备则不解码）。结果以 `ScanFile` 回流队列（**阻塞入队形成背压，满队不丢弃**——丢弃会让丢失项随快照替换永久漏扫），**消费循环与其他任务穿插应用**——交互写延迟有界（不再等整轮扫描）；`ScanEnd` 阻塞入队保证收尾必然到达：**消失对账**（所在目录已不存在、或 dirty 目录深入后未见的位置判为消失；clean 目录快照一致则位置必然还在）、快照整体替换、`items.added` 尾批冲刷、广播 `folder.changed`。**扫描窗口会话簿记**：窗口内消费侧 upsert/移动新增的位置记 `touched`（消失对账豁免——目录可能已被枚举过，后到的文件不在 seen 集）；删除/移走的位置记 `invalidated`（迟到的 ScanFile 丢弃，不复活已删位置）；ScanFile 应用前 stat 复验（哈希后文件被改写则以穿插到达的实时事件为准）。扫描中再请求扫描 → 合并参数自动补扫；遍历不完整时跳过消失对账与快照替换（防误删），最终一致由下轮对账保证
- **元数据对账（reconcile，只进不出）**：`.hawk/metadata/` 的 TOML 是唯一权威源（参与网盘同步），本机缓存与内存副本经此跟随外部变更。按文件 mtime 与 `IndexDb.source_mtime` 比对，只有变化的文件才重解析；新增/变更 → `apply_external_toml` 载入后刷新索引副本、登记注册表、发 `item.updated`；TOML 消失 → `clear_external` 清空素材参数（item 与位置由扫描决定存续）；解析失败跳过且不清空状态，下轮重试。每 100 文件经 StartupState 报一帧进度（对账可能持续很久，启动屏靠它续命）。**派生缓存自愈**：`hashes_with_missing_palette`（调色板缺失）与 `hashes_with_zero_dim`（宽高为 0，入库时解码暂时失败的遗留）纯内存扫描出的缺失项派发 worker 补齐（宽高另有读取端自愈：`item/list`/`item/skeleton` 响应中发现 0 宽高即派发；手动入口 `library/refresh_cache` 按文件夹/分类/标签/整库范围补缺失；源文件已不在的由 worker 静默跳过，删除对账收敛位置）
- **调色板批量回写（derived）**：worker 提炼结果经 PaletteJob 进入暂存（同 hash 以最新为准），≥500 条立即冲刷、滞留 2s 由一次性定时任务入队 PaletteFlush 唤醒（队列安静期也能落盘）；冲刷 = 逐条 `save_toml`（铁律：TOML 先行）→ `apply_batch` 内存 + SQLite 单事务 → 同步索引后**合并发一条 `items.updated`**——全库重提炼（版本迁移、缓存重建）时无 N 次单条事务、无事件洪峰
- **移动（fs_ops，do_move / do_dir_move）**：目标路径不可用（`.hawk` 内部/被 ignore）时整体转删除；`move_one` 只做索引 rekey 与元数据路径跟随（lib↔trash 去前缀后库内路径不变，元数据保持原路径 = 恢复目标），**不重算哈希**；目录移动批量 rekey 后 `prefs.rename_prefix` 让排序偏好跟随、`publish_transition` 广播事件、补扫新位置吸收监听遗漏的子文件事件
- **清空回收站（do_clear_trash）**：摘除全部回收站位置并清理元数据 paths；内容在库内无其他引用时删除元数据与缩略图；物理删除由 API 层在其后完成
- **进度上报**：`ScanReporter` 150ms 节流（阶段切换/总数变化强制发帧）写 StartupState（→ `/app/startup`）；`task.progress(index)` 500ms 节流 + 空闲转变化时补发清零帧

### api/item/ 详解

- **list / skeleton / detail / count**：纯查询。list/skeleton 共用 `build_query`（同一转换路径保证两次查询次序逐位一致），走 `ItemIndex::query` / `query_skeleton`；detail 走 `get_dto` 锁内投影
- **add**：`path`/`url`/`img_base64` 三选一取内容（url 经 ureq 30s 超时下载到内存，扩展名从 URL 推断、推断不出按内容嗅探；base64 必须能被 image 识别否则 `UNSUPPORTED_FORMAT`）→ 目标已存在报 `FILE_EXISTS` → **写入前先算哈希**确定 `already_existed`（避免 watcher 竞态改变语义）→ 文件落库（`spawn_blocking`，不阻塞运行时线程；path 导入保留原文件 mtime/atime）→ `submit_upsert` 携带已知哈希（流水线跳过重算，大文件免二次读盘）→ 附带的 tags/categories/annotation/website 经 `submit_metadata` 写入 → 响应取 `get_dto` 最新投影
- **upload**：multipart/form-data 内容入库（web 端用，浏览器无文件路径可引用）：`file`（文件名只取末段防跨目录，扩展名决定类型，与 path 导入同语义不校验内容）/`folder_path`/`name` → 与 add 相同的同名检查→哈希→落盘→upsert 闭环，响应同 add；写权限 viewer 需 `[web].writable`（auth 中间件统一拦截）
- **update**：经 `find_location` 取位置快照 → 回收站中的文件禁止改名/移动 → `name` 分支做真实 rename 并 `submit_move`；`folder_path` 分支**按移动后的最新位置再移动**（改名+移动同请求时基于新文件名计算目标）→ tags/star(0–5 校验)/categories/annotation/url 走 `submit_metadata` → 响应 `get_dto` 投影
- **batch_update**：校验后先逐个移动主位置（同名冲突不整体失败，跳过该项记入 missing），再 `submit_batch_metadata` 一次提交（标签/分类并集追加、评分设置）；不存在的 id 由流水线记入 `missing_ids`；无任何更新字段返回 400
- **delete / restore**：delete 把文件移入 `.hawk/trash/`（保留目录结构，冲突加 ` (n)` 后缀）；restore 按回收站实际名称去掉前缀后的路径放回。位置定位：带 `path` 走 `find_location` 单位置快照；不带 `path` 为卡片级操作，经 `item_locations` 枚举全部库内（delete）/回收站（restore）位置逐个处理——同内容多路径 item 只回收一个位置会便卡片残留；restore 同名冲突的位置跳过留在回收站，全部冲突才报 `FILE_EXISTS`
- **thumbnail**：缩略图为单一尺寸 1024 的缓存（扫描导入即时生成，增量/对账不生成，读取端兜底）：命中返回 webp；未命中且浏览器可渲染（jpg/png/gif/webp/bmp）→ **直接回源原图**（200，同时后台入队生成缓存）；不可渲染格式（tiff 等）后台生成、生成中 404（经 `item.updated` 重建）。响应 `Cache-Control: immutable`（id 是内容哈希，内容永不变）——注意未命中回源的原图响应同样 immutable，客户端可能长期持有原图字节而非升级 webp（视觉无损，接受）
- **file / thumbnail 均流式返回**（tokio 异步读 128KB 块，`Body::from_stream`）：大文件不整读进内存、不阻塞运行时线程；读中途出错以流错误终止响应（客户端感知截断）
- **file**：主位置（优先库内）原图二进制，Content-Type 按扩展名 `mime_guess`；同样 immutable、放行 `?token=`
- **refresh_thumbnail**：取可读主位置强制重建缩略图
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
    └► 后台 run_scan(false)：ScanStart 建 会话+runner 线程 → 目录快照对比 → dirty 目录深入 → 并行哈希 → ScanFile 回流穿插应用 → ScanEnd 收尾对账
         停机期间的增删改由此收敛（就绪到扫描完成之间有秒级~分钟级窗口，运行期变更由 watcher 实时覆盖；扫描期间 API 写照常受理）
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
| 行为测试 | `src/core/pipeline/tests.rs`（`cargo test`） | 索引流水线全链路：upsert 幂等、扫描消失对账、移动的同一性继承（真实临时素材库，文件系统/元数据/索引全链路） |
| OpenAPI 契约校验 | `src/api/contract_tests.rs`（`cargo test`） | 固化 openapi.json 与代码生成同步、端点全量归类、读/写端点成功路径响应体 JSON Schema 校验、SSE 事件名与载荷双向比对；CI rust job 随 `cargo test` 执行 |
| 端到端冒烟 | `tools/smoke.sh` | 临时素材库 + curl 覆盖 HTTP API 全流程（鉴权、索引、过滤、颜色检索、缩略图、去重、文件夹、监听防抖、SSE、batch_update、回收站、重启哈希复用）。语言无关的行为契约测试，需先 `cargo build --release` |

测试策略：契约级测试（HTTP/存储格式）优先于内部单元测试——行为对齐以 OpenAPI schema + `.hawk/` 存储格式 + SSE 事件契约为准。

## 工程说明

| 文件 | 说明 |
| ---- | ---- |
| `Cargo.toml` | 二进制名 `hawk-daemon`；release 配置 `lto = true` + `codegen-units = 1` + `panic = "abort"` + `strip = true`（单文件 ~9MB） |
| `openapi.json` | 契约 schema（代码生成的固化产物，同步由契约测试校验），见 server-rust.md「OpenAPI schema」节 |
| `tools/bench-*.py` | 性能压测（启动/入库/图像管线/大库全链路），输出带 git SHA 的 RESULT 行，基线见 server-rust.md |

## 排查指引

- 索引不一致 → `POST /api/v1/library/reindex` 全量重建；watcher 缓冲溢出会自动触发同样的兜底扫描；`POST /api/v1/library/rescan` 忽略目录快照强制遍历（不读文件内容）
- 缩略图 404 → 仅应出现在不可渲染格式（tiff 等）生成期间，生成完成后经 `item.updated` 自动重建；持续 404 调 `item/refresh_thumbnail`；解码失败的素材每次查看都会重试（入队以 identify 为闸）
- 派生缓存损坏 → 删除库外缓存目录（`<系统缓存>/hawk/cache/<库标识>/`）重启自动重建（TOML 是唯一权威源）
- 行为对齐以 OpenAPI schema（`/openapi/v1.json`）为准，`.hawk/` 存储格式与 SSE 事件契约（事件名、负载形状）为持久化契约

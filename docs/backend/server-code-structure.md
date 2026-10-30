# hawk-daemon 代码导读

面向新加入者的代码地图：`hawk-daemon/src/` 每个文件做什么、与谁协作，以及几条关键流程如何把代码串起来。
设计背景见 [server-rust.md](server-rust.md)（技术选型与状态）、[storage.md](storage.md)（存储格式）、
[server-rest-api-v1.md](server-rest-api-v1.md)（接口契约）、[access-control.md](access-control.md)（权限与凭证）、[architecture.md](../architecture.md)（进程模型）。

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

## 架构评估与适用边界

### 当前定位与组件结构

当前实现面向**桌面端、单用户、单素材库**。核心目标不是通用 CRUD 或多租户服务，而是在文件系统持续变化的情况下，维持素材文件、SQLite 元数据、内存索引和派生缓存的最终一致。无本地库时可退化为仅提供远程管理面与会话代理的远程查看模式。

```text
main / settings
      │
      ▼
bootstrap（组合根）──► AppState
                          │
        ┌─────────────────┼──────────────────┐
        ▼                 ▼                  ▼
  本地 Axum API       LAN Supervisor      Remote Supervisor
  鉴权 / 路由         Web / Peer / UDP     信令 / QUIC / 会话
        │
        ▼
  LibraryState（无库模式为 None）
        │
        ├── ItemIndex：内存索引，RwLock
        ├── MetadataStore：内存副本 → SQLite
        ├── 分类、标签、锁、视图偏好等 TOML 注册表
        ├── ThumbnailWorker：独立 CPU 线程池
        └── IndexPipeline：单写者消费线程
                  ▲          ▲             ▲
                  │          │             │
              Watcher      Scanner      API 写入与 worker 回流
                  │
                  ▼
              EventBus → SSE
```

查询直接读取 `ItemIndex`，排序在锁外完成，DTO 只投影分页窗口。API 写入先操作真实文件，再向流水线提交 Job；watcher、扫描和 worker 的计算结果也汇入同一消费循环。SQLite 提交成功后更新元数据内存副本，索引变更完成后通过 SSE 通知客户端。

### 设计合理性

对当前桌面端定位，这一结构是合理的，不需要整体重构：

1. **单写者流水线匹配问题本身**：集中处理 watcher 重复事件、扫描与实时写入并发、位置迁移和元数据更新，避免索引多写者竞争。幂等处理、队列溢出重扫和扫描窗口簿记都是这一模型的必要组成。
2. **计算与状态写应用分离**：遍历、哈希、图片解码和缩略图生成在扫描线程或 worker 线程执行，结果回流流水线，不让 CPU 密集任务长期占住索引消费线程。
3. **存储权威明确**：文件系统保存素材，SQLite 是素材元数据唯一权威，内存索引服务查询，缩略图是可重建缓存，TOML 只保存可同步的小型配置与注册表。
4. **本地、LAN、WAN 复用同一业务路由**：remote 主要承担网络转发和会话管理，通过 `Admin` / `Viewer` / `RemoteWriter` 区分权限，没有另建一套远程素材业务逻辑。
5. **契约与行为测试匹配架构边界**：OpenAPI 与路由同源，流水线行为测试覆盖文件系统、元数据和索引协作，SSE 客户端在断流后通过全量查询重新对齐。

### 一致性边界

当前保证的是**运行期近实时、异常情况下最终一致**，不是文件系统、SQLite 和索引之间的请求级强事务。

部分写端点先在 API 层执行 `rename` 等真实文件操作，再等待流水线提交。如果进程在两者之间退出，或提交时发生队列已满、等待超时等错误，可能出现文件已改变但 API 返回失败的窗口。文件系统操作本身也不经过单写者，并发 API 请求仍可能在文件层竞争。

后台对账扫描通常能重新收敛位置，但不会自动回滚多步骤请求已经产生的部分效果。因此演进重点是补充并发写、流水线失败和进程中断后的恢复测试，并明确各端点的部分成功语义；当前没有必要为此引入分布式事务或重型工作流框架。

### 扩展与维护边界

1. **大库存在吞吐和内存上限**：索引写入串行；查询过滤需要遍历内存索引；`MetadataStore` 与 `ItemIndex` 各保留一份元数据副本；缩略图 worker 使用无界队列。它们都是当前场景下的取舍，应通过目标素材规模的压测验证，不能视为无限扩展方案。
2. **LAN 与 remote 是后续拆分热点**：`api/lan.rs` 同时包含 HTTP、监听生命周期、UDP 发现和配对状态机；`api` 与 `remote` 在授权和隧道转发上存在双向引用。目前无需为分层形式重写，后续修改这些功能时再逐步分离 supervisor、应用逻辑和 HTTP handler。
3. **“先监听”存在数据库加载窗口**：实际启动顺序是绑定端口 → `build_state()` 内同步执行 SQLite `load_all()` → 启动 Axum serve → 流水线注水 → ready → 后台对账扫描。SQLite 加载期间端口已绑定，但 `app/startup` 尚不能响应；只有启动反馈成为明确问题时才需要调整。
4. **当前不是多用户服务器架构**：`AppState` 只承载一个库，认证以进程 token 和库级访问密钥为主，状态与索引都在单进程内。未来服务器版可以复用 HTTP 契约和部分领域流程，但多租户、认证授权、集中存储与查询扩展需要另行设计，不能仅靠替换 SQLite 或直接部署当前二进制完成。

### 演进原则

优先保持现有端到端链路稳定：先补一致性故障测试和规模压测，再依据数据处理查询、队列或内存瓶颈；LAN/remote 随功能修改渐进拆分。没有真实服务器需求前，不提前引入存储 trait、PostgreSQL、微服务或通用 actor 框架。

## 文件清单与职责

### 启动入口

| 文件 | 职责 |
| ---- | ---- |
| `src/main.rs` | 进程入口（薄壳）：只负责 `Cli::parse_args` 与子命令分发——`local` 进入 `bootstrap::run_local`，`remote` 进入 `bootstrap::run_remote`，`openapi` 进入 `api::openapi::dump()`；不包含日志、配置或组件初始化 |
| `src/bootstrap.rs` | daemon 启动流程与组件图组装（组合根）：`run_local` / `run_remote` 分别构造有库与无库 `Settings`，启动流程内初始化 tracing/panic hook；`build_state` 直接构造 `api::AppState`（单一共享状态对象，按依赖分层分段：存储底座 → 索引流水线 → 分类与视图偏好 → 缩略图 → LAN；db/store/scanner/migrator 是构造中间件不进组件图；worker.attach 回流接线须在 start 前）；`run` 按序执行：**先绑端口**（严格按 `--port`，占用即退出码 4 + `PORT_IN_USE <port>`，不静默漂移；先绑再建索引，端口冲突秒级失败）→ `build_state` → `build_router` → **先监听**（环回 `axum::serve` 优雅退出；LAN 由 supervisor 常驻任务管理，首轮回合即按配置绑定）→ `pipeline.start()`（注水/消费线程/worker/周期对账 + 周期兜底扫描定时检查）→ 接线 watcher（ConfigChanged 先 `config.reload()` 再按差异分发：ignore/扩展名白名单变化 → 强制重扫（收敛既有条目）；[web] 变化 → LAN 热重绑）→ `startup.mark_ready()` → 后台全库对账扫描 |
| `src/settings.rs` | 启动设置（解析层/配置层分离）：`Cli` 用 clap derive 声明 `local <PATH>` / `remote --remote-config <PATH>` / `openapi` 三个子命令；`local` 的库路径与 `remote` 的远程配置均为必填，模式不依赖可选参数推断。`DaemonArgs` 共享 `--port`（严格绑定，0 = 系统分配）/`--web-dist`/`--cache-parent`，各项支持同名 `HAWK_*` env 回退、CLI 优先；`Settings::from_local` / `from_remote` 承接业务校验与 env-only 参数——token 只走 `HAWK_TOKEN`（避免出现在进程列表，未传入时生成随机值并打印 stdout），`HAWK_MAINTENANCE_INTERVAL` 派生维护间隔（默认 60s，0 关闭）；周期兜底重扫在库配置 `[scan]`（periodic/interval，热更即生效） |

### api/ —— HTTP 层

| 文件 | 职责 |
| ---- | ---- |
| `src/api/lan.rs` | LAN 双监听 supervisor（网站端口与 hawk-app 直连端口独立收敛，启动即绑定并每 2s 自愈）；hawk-app 端口常驻以接收自动配对请求；发现响应器通过 UDP 27374 应答 probe；自动配对请求保存在内存中；设备密钥有独立 CRUD 端点，只操作 peer_keys、不依赖网站密钥；协议 v2 要求源端携带 reverse 回连 profile，目标端确认后动态创建独立 peer key 并在 decision 响应返回源端 profile，实现一次配对双向连接；权限变更通过 SSE lan.access.changed 通知已连接客户端。 |
| `src/api/mod.rs` | `AppState` 共享状态与 `build_router`；中间件链 cors → auth → ready_gate（axum 中后注册的 layer 在外层，请求依次经过）；auth 双 token：admin（进程 token，全权限）与 viewer（局域网访问密钥；路由器按 `ListenerMode` 分端口校验：网站端口只认 `keys` 且仅 `enabled=true`，直连端口只认 `peer_keys`；每把密钥独立 `write` 权限，`AccessLevel::Viewer { writable }` 每请求按 `current().web` 查找判定，热生效；peer 端口鉴权成功时记录该 key 最近活跃时间供在线状态端点使用），`/api/v1/events`、`item/thumbnail`、`item/file` 三个无法设请求头的 GET 端点放行 `?token=`；ready_gate 未就绪时 `/api/*` 一律 503 `NOT_READY`，仅放行 `app/startup`；全局 DefaultBodyLimit 256MB（upload 整文件与 replace 的 base64 超出 axum 默认 2MB）；最外层 CatchPanicLayer 把 handler panic 转 500 |
| `src/api/envelope.rs` | 统一成功/错误信封、错误码常量、`ApiError` → 响应的 `IntoResponse`；`JsonBody` 提取器把 JSON 解析失败统一转为 `INVALID_PARAM` |
| `src/api/app.rs` | `health`（就绪前 503，无需 token）、`startup`（starting 带进度 / ready / error）、`status`（积压快照 + 观测字段：最近扫描统计 / 队列溢出 / 配置错误 / SSE lag 次数，轮询型客户端用）、`info`（版本/平台/可执行路径/access 级别）、`token`（token 发现：Host 限定环回地址防 DNS rebinding，响应不带 CORS 头） |
| `src/api/library.rs` | `library/info` GET（显示名取 config 的 name，缺省目录名；含 `[scan]` 设置）/ PATCH（改显示名，toml_edit 保注释、广播 `library.updated`）、`library/scan` PUT（周期兜底重扫设置）、`reindex`（全量重哈希扫描，异步立即返回）、`rescan`（忽略目录快照强制遍历，仍按 size/mtime 复用哈希；带 path 时只重扫该子树——消失对账限定在范围内、快照不替换）、`refresh_cache`（按范围补派生缓存 + 消失对账）、`cleanup_index`（索引体检：清隐藏文件/ignore 或扩展名白名单外命中/源文件已删的残留位置，只摘索引不动磁盘）。素材元数据固定 SQLite，不再有 `storage_mode` |
| `src/api/global_filter.rs` | global_filter 两端点（list / PUT）。全局列表隐藏项的注册表读写（同 view 端点，不经流水线），变更广播 `global_filter.changed`（完整快照）；级联跟随在流水线对应 Job 内完成 |
| `src/api/folder.rs` | folder 五端点。list 为**目录结构缓存 + 实时计数合并**（`FolderTreeCache`：递归建树的文件系统遍历结果缓存，count 每次从索引 `folder_counts` 覆写；失效与 folder.changed 同线——API 写端点内同步失效，外部改动经总线订阅失效，ignore 配置变更亦失效）；create/update/delete/restore 先做校验（名称合法、父目录存在、`FILE_EXISTS`、禁止移入自身子目录）再做真实目录操作，变更经 `submit_dir_move` 同步索引（DirMoveJob 内广播 folder.changed，端点不重复通知） |
| `src/api/taxonomy.rs` | category/tag 八端点。list = 注册表 ∪ 全部 item 赋值并集（count 不含回收站）；名称校验在端点层，写全部 `submit_*` 给流水线执行 |
| `src/api/view.rs` | view 三端点（preferences / preference PUT / DELETE）。偏好与索引/元数据无耦合，注册表自带锁，端点直接读写，**不经过索引流水线** |
| `src/api/trash.rs` | `trash/clear`。顺序铁律：先 `submit_clear_trash` 清索引位置、元数据与缓存（缩略图/调色板），再物理删除 `.hawk/trash/` 内容——先物理删除会让 watcher 的 Deleted 事件抢先摘除位置，导致元数据与缓存泄漏 |
| `src/api/events.rs` | SSE 订阅端点：`broadcast::Receiver` 转 `event:`/`data:` 帧；lagged（消费跟不上）或总线关闭即结束流（lagged 次数累计进 app/status），客户端重连后须以 `item/skeleton` + `folder/list` 全量对齐。同文件定义 `SseEvents` 事件载荷注册表（键=事件名，与 `ItemEvents` 常量一一对应，契约测试双向比对），经 `attach_extra_schemas` 注册进 OpenAPI components |
| `src/api/openapi.rs` | OpenAPI 导出流程（`dump()` 将代码生成的 schema 打印到 stdout，不启动 daemon）、额外 schema 注册与 `/openapi/v1.json` 服务（OnceLock 缓存）；`build_openapi_json` 在 `api/mod.rs` 中统一装配路由文档、剥离 default 并序列化，schema 由代码生成并固化入库，同步性由契约测试校验 |
| `src/api/web_dist.rs` | 局域网 web 查看的静态托管（fallback 挂载；`--web-dist` 传入时启用）：SPA 回退 `index.html`，`/assets/` 内容哈希资源 immutable 长缓存，其余 no-cache（防手机浏览器启发式缓存旧 HTML）；favicon 经 vite publicDir=build/ 落在 dist 根（icon.png，index.html 内 link rel=icon 引用），由本服务一并返回。请求路径逐段校验拒绝 `..` 与反斜杠（网页版经服务端隧道暴露，本托管面对公网） |
| `src/api/item/` | item 十四端点，逻辑最重，按子域拆分（`mod.rs` 聚合路由与公共辅助；`query.rs` list/skeleton/detail/count；`add.rs` 路径导入与 URL 下载；`upload.rs` multipart 上传；`update.rs` update/batch_update；`delete.rs` 回收站进出；`file.rs` thumbnail/file/refresh_thumbnail；`replace.rs` 内容替换），见下节 |
| `src/api/contract_tests.rs`（`#[cfg(test)]`） | OpenAPI 契约校验：schema 声明的端点全量归类（新增端点未归类即失败）、空库成功路径响应体经 jsonschema 校验（$ref 经 components 提升解析）、写端点校验路由存在（区分业务错误信封与 fallback 空 404） |

### core/ —— 与 HTTP 无关的领域核心

| 文件 | 职责 |
| ---- | ---- |
| `src/core/paths.rs` | 路径规则的唯一权威：`.hawk/` 布局与 `ensure_layout`（含给 `.hawk/.gitignore` 补 `trash/` 排除项）、库内相对路径 ↔ 绝对路径互转（`..`/越界/绝对路径一律拒绝）、`is_internal`（`.hawk` 内部，回收站除外）、`is_hidden`（任一组件以 `.` 开头，回收站除外——.DS_Store/.stfolder 等隐藏项不入索引不解析，已入库残留由扫描 Remove/消失对账清理）、回收站前缀换算、`is_valid_library_path`（API 入参守卫）、库外缓存目录命名（`<库文件夹名>_<根路径 SHA-256 前16位>`，同名库靠哈希区分）、纯路径工具（`full_path` 文本归约、`dir_of`/`name_of`/`ext_of`、`unix_ms`/`file_mtime_ms`） |
| `src/core/config.rs` | `.hawk/config.toml` 快照（RwLock）与热更 `reload`（返回 `ConfigChange` 差异：ignore/extensions/web 是否变化）；库首次打开生成带注释的默认配置；ignore 匹配器：无 `/` 的模式展开为 `**/p` + `**/p/**`（任意深度同名），分段 glob（`**` 跨目录、段内 `*`/`?`），大小写不敏感；扩展名白名单 `ExtensionFilter`（空 = 不过滤；归一化去点/小写），`is_file_included` = ignore 未命中且扩展名在白名单内（文件入库的唯一判据；目录只用 is_ignored）；**解析容错**：读取/解析失败保留上次有效配置（返回空差异），错误经 `config_error()` 暴露到 app/status，修好后自动清除 |
| `src/core/metadata_db.rs` | `.hawk/metadata.db` 唯一权威 SQLite：schema v2（`items`/`paths`/`tags`/`categories`/`folders` + `meta`），v1 仅增量加入 `phash`/`phash_version`，已有数据不丢；`journal_mode=DELETE`、`busy_timeout` 5s；`load_all` 启动注水、`save`/`save_batch` 单事务/`delete`/文件夹快照。没有缓存熔断或 TOML 回退，读写失败直接上传 |
| `src/core/metadata.rs` | 元数据模型（`ItemMetadata`：paths + url/tags/star/annotation + 宽高 + `palette` + `phash`）；仅负责内存与 SQLite 之间的数据结构，不再有 TOML 解析/序列化路径。`PerceptualHash` 保存算法版本与可选 64 位值，非图像用 value=None 负缓存 |
| `src/core/metadata_store.rs` | 元数据存取：内存查询副本 + path→hash 反查表 + `.hawk/metadata.db` 唯一权威。构造直接 `MetadataDb.load_all` 注水，读取失败写 StartupState 进入错误页，不按空库降级；单条/批量写先提交 SQLite 再更新内存，`persist_batch` 同步持久化与内存并返回实际成功项供索引/事件使用；`hashes_with_missing_palette` / `hashes_with_missing_phash` / `hashes_with_zero_dim` 供派生维护。旧 TOML 不读取、不迁移，库外 index.db 与 StorageMode 已删除 |
| `src/core/item.rs` | 索引中的 item：`ItemLocation`（一个文件位置，回收站位置以 `.hawk/trash/` 开头）+ `Item`（位置列表 + 元数据查询副本 + 宽高 + 调色板，每项同时保存 RGB 与**预算 Lab** 检索坐标）；`sync_from` 为元数据 → 索引的单向同步（只允许流水线调用，palette 为 None 时保持索引现状）；`to_dto(trash_view)` 锁内投影——回收站视图的 paths 展示原库内路径（恢复目标）、folders 派生去重；`ItemQuery`/`ItemDto`/`ItemSkeletonDto` |
| `src/core/index.rs` | 内存索引：hash→item 与位置路径→hash 双字典，一把 `RwLock`。读取纪律见总览规则 3。`query` 实现 item/list 全部过滤（AND 语义）+ 排序 + 分页：锁内只做过滤与轻量排序键投影，**排序在锁外基于轻量键进行**（name 预计算小写键，避免比较器内反复分配）、主键同值按 (id, path) 字典序打破平局、desc 反转整个比较结果、`total_size` 为过滤后全量字节合计，**DTO 只投影分页窗口**（大库下不为全部命中项克隆完整 DTO）；`query_skeleton` 同过滤同排序投影 id/path/width/height/star/size（两次独立查询次序逐位一致，前端按 offset 取窗口依赖这一点）；颜色检索：查询色一次转 Lab，逐 item 检查预算 Lab 的 ΔE² ≤ 25²（免开方）；`folder_counts`（含祖先目录，同 item 同目录只计一次）/`category_counts`/`tags_with_counts` 供侧栏与 folder/list |
| `src/core/pipeline/` | **核心**：索引流水线模块（单写者 actor），按职责拆分，详见下节：`mod.rs`（Job 枚举/提交表面/消费循环/超时）、`ctx.rs`（上下文 + JobSender 外部回流通道 + 进度快照）、`upsert.rs`（入库生命周期）、`fs_ops.rs`（删除/移动/回收站）、`scan.rs`（扫描 runner + 会话簿记 + 定向扫描）、`reconcile.rs`（元数据对账）、`derived.rs`（调色板/宽高回写） |
| `src/core/content_hash.rs` | BLAKE3 流式哈希（1MB 缓冲），即 item id，元数据/缩略图命名依据 |
| `src/core/perceptual_hash.rs` | 64 位 pHash：`image_hasher` DCT + Mean、稳定 Lanczos3；`PHASH_VERSION=1`；XOR 汉明距离。单元测试覆盖重复计算、缩放、JPEG 重压缩与不同画面 |
| `src/core/duplicates.rs` | 重复分组：exact 检查同 item 多位置；similar 先按 bits 预分组，4×16bit 分带索引（≤10 保证不漏候选）+ 完整距离复核 + 并查集，>10 回退 BK-tree。分页稳定排序；与暴力分组对照测试及 100k 性能基准 |
| `src/core/events.rs` | SSE 事件总线：tokio broadcast（容量 1024），订阅者消费跟不上（lagged）即由端点断开其订阅；`TaskProgress` 快照结构（task.progress 事件与 app/status 共用）；`folder.changed` 负载构造 |
| `src/core/fs_util.rs` | 文件操作辅助：建父目录、回收站冲突追加 ` (n)` 后缀（恢复按实际名称放回）、名称合法性校验（禁分隔符/`.`/`..`/`.hawk`） |
| `src/core/thumbnail.rs` | 图像服务：`identify`（只解码头取尺寸）、`detect_extension_bytes`（guess_format → 扩展名映射，item/add 类型推断用）、`is_probably_image`（只读头部 64B 内容喷探，非图像文件在解码派生前拦下；打不开时保守视为图像）、`generate`（固定 512/1024 两档，Max 等比缩小**不放大**、Lanczos3、有损 WebP q80、单次解码生成全部缺失档/force 强制重建、仅缺 512 时复用已有 1024 缓存解码、按内容寻址存库外缓存目录、非图像跳过）、`delete`（清理全部尺寸）。解码失败记 warn 并返回 false（读取端重试自愈） |
| `src/core/color.rs` | 调色板提炼：降采样 64px（Triangle 滤波）→ **median-cut** 量化 ≤10 色 → 像素占比（alpha<128 不参与，0–100 保留 1 位小数）；按占比降序、并列按 RGB 升序保证确定性 |
| `src/core/color_math.rs` | 颜色纯函数：hex 解析/格式化、sRGB→CIELAB（D65）、CIE76 ΔE 的**平方**（与阈值平方比较免开方） |
| `src/core/thumbnail_worker.rs` | 缩略图/派生后台 worker：**无界** mpsc 队列（in-flight 去重）+ `CPU/2`（封顶 12）个专用 OS 线程。`enqueue_derived`（入库/维护：单次解码补调色板 + pHash + 宽高）、`enqueue_thumbs`（读取端未命中/范围刷新：再补缩略图）、`enqueue_force_rebuild`（强制重建）。非图像写调色板/pHash 负缓存终止重试；暂时解码失败不写负缓存。结果经 JobSender 回流消费循环，派生批量落 SQLite；生成/修复缩略图后补发 `item.updated`，单任务 `catch_unwind` 隔离，积压经 task.progress 500ms 节流可见 |
| `src/core/taxonomy.rs` | 分类/标签维度：`normalize_category_name`（扁平，无层级）；注册表骨架（固定 schema `key = [字符串数组]`，读写公共件在 `registry_file.rs`，重命名合并语义）→ `CategoryRegistry`/`TagRegistry`；`ItemEvents`：SSE 事件名常量与发布辅助（`publish_changed`/`publish_location_loss`/`publish_transition`，位置归零 → removed、只剩回收站 → trashed、首个库内位置回归 → restored）；`TaxonomyMigrator`：元数据写应用与全库级联迁移，**只被消费循环调用**；批量/级联统一走 `finish_metadata_batch`（空操作跳过、分片并行落盘、`ChangeBatcher` 合并 `items.updated` 事件） |
| `src/core/view_prefs.rs` | 视图偏好注册表（`.hawk/view.toml`，参与同步）：扁平 map（scope 键 `folder:<路径>`/`category:<名>`/`tag:<名>`），不理解继承语义（前端沿父链解析）；scope/排序值白名单校验；`rename_prefix`/`delete_prefix` 跟随目录移动/删除；外部修改（含网盘同步落地）经 watcher 触发 `reload` |
| `src/core/global_filter.rs` | 全局列表隐藏项注册表（`.hawk/global_filter.toml`，参与同步）：folders/categories/tags 三个列表；端点直接读写（同 view_prefs，不过流水线），级联跟随（文件夹移动/删除、分类/标签改名删除）由流水线对应 Job 调用；变更广播 `global_filter.changed` |
| `src/core/registry_file.rs` | 注册表文件持久化公共件：原子写（临时文件 + rename）、字符串列表键解析（trim/去空/去重/小写排序）与格式化；taxonomy/global_filter/view_prefs 共用 |
| `src/core/scanner.rs` | 目录遍历（只读目录项，不读文件内容）：`walk_directory`（跳过 `.hawk` 内部、只深入 trash 子树、库内应用 ignore）；`walk_directory_stats`（产出 目录 → (mtime, 直接子项数) 供增量扫描快照对比，隐藏目录不深入不进结果——消失对账据此清理其下残留位置，枚举失败置 `walk_incomplete`——调用方据此跳过消失对账防误删）；`walk_files_in_directory`（增量深入时只枚举直接文件，隐藏文件交给 upsert 判定 Remove 清理）；文件枚举统一经 `config.is_file_included`（ignore + 扩展名白名单），回收站子树不参与过滤 |
| `src/core/watcher.rs` | 文件监听（notify 封装，持有配置用于白名单过滤）：原生粒度事件折叠为 FileSystemWatcher 语义（Create → upsert；Data/Metadata 修改 → 文件 upsert；Remove → Deleted；目录 → FolderCreated）；rename 的 From/To **配对带 300ms 超时兜底**（滞留 From 按删除处理，150ms ticker 周期 flush + 事件到达顺带 flush；并发多 rename 错配由幂等流水线 + 超时兜底自愈）；**macOS FSEvents 的 rename 只有单路径 `Name(Any)`（旧/新各发一条）**：以磁盘现状定端——路径已不在 = 旧端（进配对等待），存在 = 新端（配对为 Moved，无旧端则 upsert）——临时文件 + rename 落盘（Finder/ditto 复制、编辑器原子保存）的新名字由此入库；config.toml / categories.toml / tags.toml / view.toml 单独上报驱动热更；缓冲 Overflow → 全库强制遍历兜底；`Flag::Rescan`（FSEvents 明确告知某路径丢事件）→ 定向强制重扫该路径（路径不可用时退回全库兜底）；`.hawk` 内部（回收站除外）与扩展名白名单外的文件不产生索引事件 |
| `src/core/startup.rs` | 启动状态：进度快照（phase/processed/total）、就绪标志、失败原因。`/health`、就绪网关与 `app/startup` 端点的共同数据源 |

### IndexPipeline 详解

所有索引变更收敛为单消费者循环（专用线程 `hawk-index-pipeline`），Job 经有界 channel（4096）串行处理：

- **Job 类型**：`Upsert` / `Delete` / `Move` / `DirMove` / `ScanStart`+`ScanFile`+`ScanEnd`（扫描三段，见下；`ScanStart` 带 `scope` 时只遍历该子树，在途定向请求排队补扫）/ `ClearTrash` / `Metadata` / `BatchMetadata` / `MetadataSync` / `Palette` + `PaletteFlush`（调色板批量回写）/ `FixDim`（宽高补全）/ `FolderHint` / `CategoryCreate|Update|Delete` / `TagCreate|Update|Delete` / `RegistryReload`
- **两类入口**：watcher/worker 走 fire-and-forget（经 `JobSender`），channel 满置 overflow 标记（消费者检查后入队**去重的扫描**兜底，不内联扫描，避免事件风暴期反复全库遍历）；API 与启动走携带 `oneshot` 回复通道的提交，统一 60s 超时等待（`run_scan` 例外——等整轮扫描完成，不设超时），消费循环停止/任务 panic 时调用方立即收到错误而非挂起。消费循环对每个 Job 做 `catch_unwind`，panic 的任务被跳过
- **入库（do_upsert）拆两步**：`prepare_upsert`（纯判定、无副作用——路径过滤 `.hawk`/ignore/扩展名白名单外、stat、**哈希复用判定**（路径与 size/mtime 同元数据一致即复用不读内容）、写入中文件防抖（mtime 距今不足 1s 延迟重试，同路径去重上限 120 次）；需要删除路径时返回 Remove 由调用方执行，消费循环内联、扫描 runner 经队列回流）→ 哈希（`known_hash` 直接采用 / 复用 / 计算）→ **复验 size/mtime**（哈希期间仍在写入则延迟重试，不以半截内容入库，慢速拷贝的哈希漂移由此根治）→ `apply_upsert`（串行应用；创建 item 与登记位置在同一锁内完成，零位置 item 不对并发查询可见）
- **`apply_upsert`**：哈希漂移时先取旧元数据用于继承，再按路径迁移（新 item 继承 tags 等素材参数，旧元数据无引用则删除元数据与缩略图）并发布位置丢失事件；元数据登记路径并回写最新 size/mtime；扫描路径单次解码产出的调色板与 pHash 随首版 SQLite 记录一并持久化；**宽高在入库时即持久化入 SQLite**；索引同步后发事件——扫描路径合并进 `items.added` 批量事件（300ms 窗口/2000 条上限/扫描结束兜底冲刷），单条路径发即时 `item.added`/`item.updated`；`needs_palette_work` / `needs_phash_work` 任一成立即派发派生任务兜底增量与解码失败自愈——扫描导入的缩略图已在并行阶段生成，读取端惰性兜底仍在
- **全库扫描（三段任务 + 目录快照，消费线程不被长扫描独占）**：`ScanStart` 只建会话并 spawn **runner 线程**（`hawk-scan`）——阶段一 `walk_directory_stats` 与上轮 `folders` 快照（mtime + 直接子项数）对比，**只有 dirty 目录才深入文件级访问**（clean 目录不碰文件系统）；阶段二 dirty 目录枚举**直接文件**（目录项跳过；同名「文件→目录」替换由消失对账收敛）做复用判定，复用项直接回流；阶段三需哈希文件进**并行导入通道**（物理核估计（逻辑核/2，封顶 16）：解码/编码 SIMD 密集，SMT 无增益，SMT 兄弟线程留给消费循环与 API）：哈希（后复验，仍在写入的延迟重试）→ 单次解码同出 调色板/缩略图/宽高（派生齐备则不解码）。结果以 `ScanFile` 回流队列（**阻塞入队形成背压，满队不丢弃**——丢弃会让丢失项随快照替换永久漏扫），**消费循环与其他任务穿插应用**——交互写延迟有界（不再等整轮扫描）；`ScanEnd` 阻塞入队保证收尾必然到达：**消失对账**（所在目录已不存在、或 dirty 目录深入后未见的位置判为消失；clean 目录快照一致则位置必然还在）、快照整体替换、`items.added` 尾批冲刷、广播 `folder.changed`。**扫描窗口会话簿记**：窗口内消费侧 upsert/移动新增的位置记 `touched`（消失对账豁免——目录可能已被枚举过，后到的文件不在 seen 集）；删除/移走的位置记 `invalidated`（迟到的 ScanFile 丢弃，不复活已删位置）；ScanFile 应用前 stat 复验（哈希后文件被改写则以穿插到达的实时事件为准）。扫描中再请求扫描 → 合并参数自动补扫；遍历不完整时跳过消失对账与快照替换（防误删），最终一致由下轮对账保证
- **派生维护（maintenance）**：SQLite 是唯一元数据来源，不再有 TOML mtime 对账。启动与周期任务仅做纯内存缺失集扫描：`hashes_with_missing_palette` / `hashes_with_missing_phash` / `hashes_with_zero_dim`，经 worker 补齐调色板、pHash 与宽高；非图像负缓存跳过，源文件已消失的项由扫描收敛
- **派生批量回写（derived）**：worker 的调色板/pHash 结果分别暂存（同 hash 以最新为准），≥500 条立即冲刷、滞留 2s 由 DerivedFlush 唤醒；每类分别经 SQLite 单事务 + 内存/索引同步后**合并发 items.updated**——老库补算时无 N 次单条事务与事件洪峰
- **移动（fs_ops，do_move / do_dir_move）**：目标路径不可用（`.hawk` 内部/被 ignore）时整体转删除；`move_one` 只做索引 rekey 与元数据路径跟随（lib↔trash 去前缀后库内路径不变，元数据保持原路径 = 恢复目标），**不重算哈希**；目录移动批量 rekey 后 `prefs.rename_prefix` 让排序偏好跟随、`publish_transition` 广播事件、补扫新位置吸收监听遗漏的子文件事件
- **清空回收站（do_clear_trash）**：摘除全部回收站位置并清理元数据 paths；内容在库内无其他引用时删除元数据与缩略图；物理删除由 API 层在其后完成
- **进度上报**：`ScanReporter` 150ms 节流（阶段切换/总数变化强制发帧）写 StartupState（→ `/app/startup`）；`task.progress(index)` 500ms 节流 + 空闲转变化时补发清零帧

### api/item/ 详解

- **list / skeleton / detail / count**：纯查询。list/skeleton 共用 `build_query`（同一转换路径保证两次查询次序逐位一致），走 `ItemIndex::query` / `query_skeleton`；detail 走 `get_dto` 锁内投影。`similar_to` + `max_distance` 启用 pHash 过滤，`order_by=similarity` 按距离排序并投影 `similarity_distance`；源图锁与分析状态在 API 层校验。`item/duplicates`（admin）复用同一 pHash 候选做分组扫描，分页返回各位置的 Item 与差异值
- **add**：`path`/`img_base64` 二选一取内容（入库策略前置校验：ignore 规则/扩展名白名单外的目标直接 4xx，不落盘）（base64 必须能被 image 识别否则 `UNSUPPORTED_FORMAT`；图片的网络下载由浏览器插件完成后以 base64 提交，服务端不再自行下载——无代理无会话、TLS 指纹常被拒，能力是浏览器网络栈的子集）→ 目标已存在报 `FILE_EXISTS` → **写入前先算哈希**确定 `already_existed`（避免 watcher 竞态改变语义）→ 文件落库（`spawn_blocking`，不阻塞运行时线程；path 导入保留原文件 mtime/atime）→ `submit_upsert` 携带已知哈希（流水线跳过重算，大文件免二次读盘）→ 附带的 tags/categories/annotation/website 经 `submit_metadata` 写入 → 响应取 `get_dto` 最新投影
- **upload**：multipart/form-data 内容入库（web 端用，浏览器无文件路径可引用）：`file`（文件名只取末段防跨目录，扩展名决定类型，与 path 导入同语义不校验内容）/`folder_path`/`name` → 与 add 相同的同名检查→哈希→落盘→upsert 闭环，响应同 add；写权限 viewer 需对应访问密钥 `write=true`（auth 中间件统一拦截）
- **update**：经 `find_location` 取位置快照 → 回收站中的文件禁止改名/移动 → `name` 分支做真实 rename 并 `submit_move`；`folder_path` 分支**按移动后的最新位置再移动**（改名+移动同请求时基于新文件名计算目标）→ tags/star(0–5 校验)/categories/annotation/url 走 `submit_metadata` → 响应 `get_dto` 投影
- **batch_update**：校验后先逐个移动主位置（同名冲突不整体失败，跳过该项记入 missing），再 `submit_batch_metadata` 一次提交（标签/分类并集追加、评分设置）；不存在的 id 由流水线记入 `missing_ids`；无任何更新字段返回 400
- **delete / restore**：delete 把文件移入 `.hawk/trash/`（保留目录结构，冲突加 ` (n)` 后缀）；restore 按回收站实际名称去掉前缀后的路径放回。位置定位：带 `path` 走 `find_location` 单位置快照；不带 `path` 为卡片级操作，经 `item_locations` 枚举全部库内（delete）/回收站（restore）位置逐个处理——同内容多路径 item 只回收一个位置会便卡片残留；restore 同名冲突的位置跳过留在回收站，全部冲突才报 `FILE_EXISTS`
- **thumbnail**：固定 512/1024 两档 WebP 缓存（扫描导入即时生成全部缺失尺寸，增量/对账不生成，读取端兜底）：`size` 缺省 1024，其他尺寸 400；命中请求尺寸返回对应 WebP + `Cache-Control: immutable`；未命中返回 404 并后台入队生成，完成经 `item.updated` 重建。端点**不回源原图**，避免低带宽按卡片下载源文件及 immutable 缓存钉住回退响应
- **file / thumbnail 均流式返回**（tokio 异步读 128KB 块，`Body::from_stream`）：大文件不整读进内存、不阻塞运行时线程；读中途出错以流错误终止响应（客户端感知截断）
- **file**：主位置（优先库内）原图二进制，Content-Type 按扩展名 `mime_guess`；同样 immutable、放行 `?token=`
- **refresh_thumbnail**：取可读主位置强制重建缩略图
- **replace**（`item/replace`）：客户端编辑（旋转/裁切）后的新内容提交存储层。内容必须可识别且**格式与文件扩展名一致**（扩展名与内容错位会破坏类型推断与预览）；哈希相同则幂等直接返回当前投影；写回保留原 mtime（修正性编辑不改变素材的时序位置）；`submit_upsert` 触发 id 漂移闭环（元数据继承迁移/事件/缩略图重建）

## 关键流程串联

### 1. 启动建索引（先监听、后索引）

```text
绑定 127.0.0.1（+ 可选 LAN）→ axum::serve 拉起（startup 端点已可答 starting）
    └► MetadataStore 构造即注水：metadata.db 直接 load_all（读取失败进入启动错误页）
    └► hydrate_index：元数据副本 → 内存索引（秒级）
    └► pipeline.start()：消费线程 + ThumbnailWorker + 周期派生维护 + 首轮 DerivedMaintenance（先于扫描）
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

### 3. 派生数据补全（老库 / 算法升级 / 增量素材）

```text
启动与周期维护（默认 60s）→ 纯内存扫描 palette/phash/宽高缺失集
  → worker 单次解码按需补齐 → JobSender 回流
  → 每 500 条或 2s 批量写 metadata.db → 合并 items.updated
  → 相似查询与颜色查询立即可用；缩略图仍按读取端惰性生成
```

### 4. item/add

```text
api: 取内容（复制/解码）→ 预计算哈希 → 文件写入库内
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
| 单元测试 | `cargo test`（各文件内联 `#[cfg(test)]`） | 纯函数与纯数据结构：颜色数学（hex/Lab/ΔE 已知向量）、median-cut 提炼（纯色→1 色 ~100%、双色各半、全透明→空、占比合计）、pHash 缩放/重压缩/汉明距离、SQLite v1 加列保留、ignore 匹配、路径换算与回收站前缀、BLAKE3 标准向量、视图偏好校验 |
| 行为测试 | `src/core/pipeline/tests.rs`（`cargo test`） | 索引流水线全链路：upsert 幂等、扫描消失对账、移动的同一性继承（真实临时素材库，文件系统/元数据/索引全链路） |
| OpenAPI 契约校验 | `src/api/contract_tests.rs`（`cargo test`） | 固化 openapi.json 与代码生成同步、端点全量归类、读/写端点成功路径响应体 JSON Schema 校验、SSE 事件名与载荷双向比对；CI rust job 随 `cargo test` 执行 |
| 端到端冒烟 | `tools/smoke.sh` | 临时素材库 + curl 覆盖 HTTP API 全流程（鉴权、索引、过滤、颜色检索、缩略图、去重、文件夹、监听防抖、SSE、batch_update、回收站、重启哈希复用）。语言无关的行为契约测试，需先 `cargo build --release` |

测试策略：契约级测试（HTTP/存储格式）优先于内部单元测试——行为对齐以 OpenAPI schema + `.hawk/` 存储格式 + SSE 事件契约为准。

## 工程说明

| 文件 | 说明 |
| ---- | ---- |
| `Cargo.toml` | 二进制名 `hawk-daemon`；release 配置 `lto = true` + `codegen-units = 1` + `strip = true`（单文件 ~9MB）。**不启用 `panic = "abort"`**：保留 unwind 才能让消费循环/扫描 runner/worker 的 `catch_unwind` 隔离真正生效（abort 下 panic 直接终止进程，隔离形同虚设） |
| `openapi.json` | 契约 schema（代码生成的固化产物，同步由契约测试校验），见 server-rust.md「OpenAPI schema」节 |
| `tools/bench-*.py` | 性能压测（启动/入库/图像管线/大库全链路），输出带 git SHA 的 RESULT 行，基线见 server-rust.md |

## 排查指引

- 索引不一致 → `POST /api/v1/library/reindex` 全量重建；watcher 缓冲溢出会自动触发同样的兜底扫描；`POST /api/v1/library/rescan` 忽略目录快照强制遍历（不读文件内容）
- 缩略图 404 → 仅应出现在不可渲染格式（tiff 等）生成期间，生成完成后经 `item.updated` 自动重建；持续 404 调 `item/refresh_thumbnail`；解码失败的素材每次查看都会重试（入队以 identify 为闸）
- 缩略图缓存损坏 → 删除库外缓存目录（`<系统缓存>/hawk/cache/<库标识>/`）重启自动重建；metadata.db 是唯一元数据权威，不能靠删除它修复查询
- 行为对齐以 OpenAPI schema（`/openapi/v1.json`）为准，`.hawk/metadata.db` SQLite 格式与 SSE 事件契约（事件名、负载形状）为持久化契约

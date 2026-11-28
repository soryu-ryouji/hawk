# hawk-daemon 代码导读（Go）

面向新加入者的代码地图：`hawk-daemon/` 每个包/文件做什么、与谁协作，以及关键流程如何串起来。
设计背景见 [server-go.md](server-go.md)、[storage.md](storage.md)、[server-rest-api-v1.md](server-rest-api-v1.md)、
[access-control.md](access-control.md)、[architecture.md](../architecture.md)。

## 总览

```text
main.go（CLI / 启动 / 信号）
  └─ internal/library（领域核心：库、索引、扫描、监听、缩略图、注册表、锁）
  └─ internal/auth   （AccessKey、Web Session、本机集成 Session）
  └─ internal/api    （HTTP 路由、鉴权、SSE、局域网 supervisor/发现/配对）
```

依赖方向：`api` → `library` / `auth`；`auth` → `library`（角色/通道类型）；`library` 不依赖 HTTP。

三条硬规则：

1. **索引与元数据的写入由 `Library.writeMu` 串行化**。API、扫描、监听、缩略图 worker 都经同一把写锁应用变更；读取只拿 `Index` 的读锁。
2. **处理幂等**。同一文件事件重复到达不会产生副作用；扫描是最终一致兜底。
3. **锁内投影，锁外排序**：查询在索引读锁内复制轻量快照，排序与 DTO 投影在锁外进行。

## 文件清单与职责

### 启动

| 文件 | 职责 |
| ---- | ---- |
| `main.go` | CLI（`local` / `openapi`）、参数与环境变量、打开库、启动 LAN supervisor、严格绑定本地端口、优雅退出 |
| `internal/library/library.go` | `Library` 组件图、启动/关闭、启动状态、事件发布辅助、路径抑制（API 文件操作防 watcher 抢跑） |
| `internal/library/paths.go` | `.hawk/` 布局、相对/绝对路径换算与安全校验、库标识、缓存目录命名 |
| `internal/library/config.go` | `config.toml` 读取/热更、ignore 匹配器、扩展名白名单 |
| `internal/library/sqlite.go` | `metadata.db` schema v3、`load_all`、批量写、目录快照 |
| `internal/library/types.go` | Role/Channel/Capability、Item、DTO、查询条件 |
| `internal/library/index.go` | 内存索引：hash→item、路径→hash、查询/计数/位置快照 |
| `internal/library/query.go` | `item/list`/`skeleton` 的过滤、排序、分页与 DTO 投影 |
| `internal/library/scan.go` | 全库/子树扫描、并行哈希、应用结果、消失对账 |
| `internal/library/watcher.go` | fsnotify 递归监听、防抖、rename 配对、配置/注册表热更 |
| `internal/library/upsert.go` | 单文件入库/移动/删除、元数据应用、回收站、索引体检 |
| `internal/library/thumbnail.go` | WebP 512/1024 生成、缩略图 worker、调色板/宽高回写 |
| `internal/library/color.go` | CIELAB、ΔE、median-cut 调色板 |
| `internal/library/taxonomy.go` / `taxonomy_ops.go` | 分类/标签注册表与级联迁移 |
| `internal/library/views.go` | 视图排序偏好（`view.toml`） |
| `internal/library/filter.go` | 全局隐藏项（`global_filter.toml`） |
| `internal/library/locks.go` | 文件夹/分类/标签锁、Argon2id、解锁票据、请求级守卫 |
| `internal/library/events.go` | SSE 事件总线、事件名常量、进度快照 |
| `internal/library/duplicates.go` | 重复内容分组 |
| `internal/library/fs_util.go` / `hash.go` | 文件名/回收站路径辅助、BLAKE3/格式探测 |

### HTTP

| 文件 | 职责 |
| ---- | ---- |
| `internal/api/api.go` | `Server`、路由表、CORS/鉴权/就绪网关/body limit/panic 恢复、能力表、信封与错误 |
| `internal/api/app.go` | health/startup/status/info/admin-token/session/connection |
| `internal/api/library.go` | library info/rename/scan/reindex/rescan/refresh_cache/cleanup_index |
| `internal/api/folder.go` | 文件夹树与创建/重命名/删除/恢复 |
| `internal/api/item_query.go` | item list/skeleton/detail/count/aggregate/duplicates |
| `internal/api/item_write.go` | item add/upload/update/batch_update/delete/restore/replace |
| `internal/api/item_file.go` | thumbnail/file/refresh_thumbnail |
| `internal/api/taxonomy.go` | category/tag/view/global_filter |
| `internal/api/locks.go` | 锁列表/设锁/解除/解锁与票据解析 |
| `internal/api/trash.go` | 清空回收站 |
| `internal/api/events.go` | SSE 订阅 |
| `internal/api/static.go` | 局域网 Web 静态托管（SPA 回退） |
| `internal/api/openapi.go` | `go:embed` schema 服务 |
| `internal/api/lan.go` | Web/Peer 双监听 supervisor、UDP 发现、配对请求、设备密钥、在线状态 |
| `internal/api/socket_*.go` | 平台 UDP broadcast 选项 |

### 鉴权

| 文件 | 职责 |
| ---- | ---- |
| `internal/auth/auth.go` | AccessKey（`keys.json`，只存 sha256）、Web Session、本机集成 Session、角色/通道能力交集、本机身份 |

## 关键流程

### 启动

```text
main: 打开 Library（建 .hawk、读 config、SQLite load_all、内存索引注水）
  → start worker / watcher / 周期维护（Startup 立即 ready）
  → 启动本地 HTTP 127.0.0.1:<port>
  → LAN supervisor 首轮收敛（Web 监听按配置，Peer 监听常驻）
  → 后台初始扫描：遍历 → 并行哈希 → 应用 → 消失对账
```

### 外部文件变更

```text
fsnotify → 防抖/rename 配对 → UpsertPath/DeletePath/MovePath
  → writeMu 内更新索引与 SQLite → SSE item.* / folder.changed
```

扫描作为监听漏事件的兜底；队列溢出用全库扫描收敛。

### API 写入

```text
handler 校验参数 → 真实文件操作（写盘/rename，先 SuppressPath 抑制 watcher）
  → Library 写方法（writeMu 内更新索引、SQLite）
  → SSE 事件 → 响应最新 DTO
```

### 缩略图/调色板

```text
入库/读取未命中 → derive 队列
  → worker 解码一次 → 生成 512/1024 WebP + median-cut 调色板 + 宽高
  → writeMu 内写回 SQLite/索引 → item.updated
```

### 局域网连接

```text
Peer 端口 Bearer AccessKey → auth 解析 → 能力表执法 → 素材 API
Web 端口 AccessKey 换 HttpOnly Cookie → 浏览器静态页 + API
桌面配对：UDP 发现 → POST pair-requests → 目标端 admin 确认 → 签发 AccessKey
```

## 测试

| 层 | 位置 | 说明 |
| ---- | ---- | ---- |
| 单元/行为 | `internal/library/*_test.go` | 路径、ignore、调色板、扫描/移动/删除、锁 |
| HTTP 行为 | `internal/api/api_test.go` | item 生命周期、LAN 密钥/Web Session/Peer、配对 |
| 契约 | `internal/api/openapi_test.go` | 嵌入 schema 与仓库根同步、无 WAN 路径 |
| 冒烟 | `tools/smoke.sh` | 临时库 + curl 覆盖主要 HTTP 流程 |

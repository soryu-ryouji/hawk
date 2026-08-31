# hawk-server 代码导读

> 本文档原为 C# 过渡实现（`hawk-server/`）的逐文件导读。C# 版已完成使命并从仓库移除；
> 以下保留与语言无关的分层规则与关键流程说明（Rust 版 `hawk-server-rs/` 模块一一对应），
> 逐文件的 Rust 导读待补充。实现细节、状态与压测基线见 [hawk-server-rs](server-rust.md)。

设计背景：[storage.md](storage.md)（存储格式）、[server-rest-api-v1.md](server-rest-api-v1.md)（接口契约）、
[architecture.md](../architecture.md)（进程模型）。

## 分层与硬规则

hawk-server 分两层，依赖单向：`Api/`（HTTP 层）→ `Core/`（领域核心）。

三条硬规则：

1. **索引与元数据的写入只发生在 IndexPipeline 的消费循环里**（单写者）。Api 层做真实文件操作（移动/复制），随后把变更作为任务提交给流水线并等待完成；HTTP 线程只读索引。
2. **流水线的所有处理幂等**。同一事件重复到达（例如 API 主动移动文件后，watcher 又上报一次）不会产生副作用，这是免锁设计成立的前提。
3. **锁内投影，锁外只见 DTO**：HTTP 层一律走锁内投影 / 不可变快照查询，锁外不得持有或遍历可变索引项。

## 关键流程串联

### 1. 启动建索引

```text
watcher.Start()          事件开始入队（channel 缓冲）
    └► 后台对账扫描      逐文件 upsert
         路径+size/mtime 命中元数据 → 复用哈希（不读内容，仅读取尺寸）
         否则算哈希并登记元数据
    └► HTTP 先监听（app/startup 可查询进度，/api/* 503 NOT_READY）
    └► 内存索引注水完成 → ready → /health 200、API 放行（秒级）
    └► 后台对账扫描 → 停机期间增删改收敛（task.progress(index) 可见）
```

先监听、后索引（启动模型见 architecture.md）：缓冲的 watcher 事件排在扫描任务之后，被幂等处理自然去重。

### 2. 外部文件变更（用户在文件管理器里操作）

```text
LibraryWatcher → upsert/deleted/moved 事件
  → pipeline 入队 → 防抖（写入中文件延迟重试）→ 哈希/迁移/索引更新 → EventBus → SSE → 前端增量刷新
```

### 3. item/add

```text
Api: 取内容（复制/下载/解码）→ 预计算哈希 → 文件写入库内
   → 提交 upsert（携带已知哈希，流水线跳过重算）→ 提交元数据（素材参数）
   → 返回 item + already_existed
watcher 随后上报的 Created 事件幂等吸收
```

### 4. 回收站三段操作

```text
item/delete:   移入 .hawk/trash/<原路径>（冲突加后缀）→ move job
               元数据不动（paths 仍是原路径 = 恢复目标）
item/restore:  移回原路径（占用 → FILE_EXISTS）→ move job
trash/clear:   物理删除 → clear-trash job 清位置、清元数据路径；
               内容无其他引用时删元数据与缩略图
```

## 排查指引

- 索引不一致 → `POST /api/v1/library/reindex` 全量重建；watcher 缓冲溢出会自动触发同样的兜底扫描
- 缩略图 404 → 属首次索引期间的正常状态，否则调 `item/refresh_thumbnail`
- 行为对齐以 OpenAPI schema 为准（`/openapi/v1.json`），`.hawk/` 存储格式与 SSE 事件契约（事件名、负载形状）为持久化契约

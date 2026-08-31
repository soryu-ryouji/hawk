# hawk-server-rs（Rust 实现）

> 逐文件的代码职责见 [代码导读](server-code-structure.md)（C# 版结构同理，Rust 版模块一一对应）。
> API 契约见 [REST API V1](server-rest-api-v1.md)，存储格式见 [storage.md](storage.md)。

hawk-server 的 Rust 实现（`hawk-server-rs/`），替换目标为 C# 过渡版（`hawk-server/`，验证期并行保留）。
行为对齐基准 = OpenAPI schema + `.hawk/` 存储格式 + SSE 事件契约，不逐行翻译 C# 代码。

## 状态

- **app 默认后端**：`hawk-app` 的开发态（`resolveServerCommand`）、打包（`scripts/build-server.mjs`）、
  CI（`release.yml`）全部使用本实现；C# 版保留在仓库内仅供回归对比（`tools/smoke.sh csharp`），
  充分验证后移除
- **已实现**：全部 REST API、SSE、文件监听、索引流水线（防抖/并行哈希/对账扫描）、缩略图（libwebp q80）、
  调色板（median-cut，palette_version=2）、SQLite 派生缓存（与 C# v1 schema 兼容）
- **已知有意差异**（对项目更好，不逐字复刻 C#）：
  - 宽高在入库时即持久化入 TOML（C# 只在内存更新，重启后靠扫描重新识别——与 storage.md 设计意图不符）
  - `palette_version` 与标量同列于 `[[paths]]` 之前（C# 放在其后，TOML 语义上会被解析进 paths 表内，实际读不回来）
  - 调色板算法为 median-cut（v2）；C# 的 Wu（v1）结果会被视为旧版本，由后台 worker 一次性重提炼
  - 并发度高于 C# 版：哈希并行 `CPU-1`（封顶 24，C# 为 `CPU/2` 封顶 16），缩略图 worker `CPU/2` 封顶 12（C# 为 `CPU/4` 封顶 8 且 BelowNormal 优先级）——桌面端大批量导入/重建索引时吞吐优先，API 让出 1 核
  - 查询路径不克隆完整 DTO：排序在轻量键上进行，`item/list` 只投影分页窗口、`item/skeleton` 只投影轻量骨架（大库下 skeleton 持锁时间降低一个量级，UI 重连重同步不再卡死读路径）
  - 调色板回写按批冲刷（≥500 条或滞留 2s）：TOML 逐条落盘、SQLite 单事务、事件按批平滑补发——全库重提炼（v1→v2 迁移、缓存重建）时无 `item.updated` 洪峰
  - 缩略图 worker 无界队列 + 周期对账自愈：C# 版「队列满静默丢弃」在批量入库时曾永久丢失 20%+ 素材的派生缓存（24k 库实测丢 5.6k）；Rust 版队列无界（任务 ~100B、in-flight 去重后内存有界），周期对账扫描 palette 缺失项并派发 worker 补齐（完成后补发 `item.updated`，前端占位自动重建）
  - 启动注水经 TOML 全量回退时上报进度（每 1000 文件一帧，phase=sync），大库首次启动启动屏有实时反馈

## 技术选型

| 职责 | 选型 | 备注 |
| ---- | ---- | ---- |
| HTTP 框架 | axum 0.8 | tower 中间件链等价 C# 中间件顺序 |
| 异步运行时 | tokio | pipeline 消费循环为专用 OS 线程（阻塞扫描不占运行时线程） |
| 文件监听 | notify 8 | From/To rename 配对 + 300ms 超时兜底 |
| 哈希 | blake3 | item id = BLAKE3 hex（存储契约） |
| 图像解码/缩放 | image 0.25 + fast_image_resize | Lanczos3 |
| WebP 编码 | webp（libwebp） | 有损 q80，对齐 C# ImageSharp 行为；纯 Rust 的 image-webp 仅支持无损 |
| 元数据缓存 | rusqlite（bundled） | DDL 与 C# v1 逐字一致，现有缓存直接可读 |
| TOML | toml（解析）+ 手写序列化 | 输出格式与 C# 逐字对齐（含修正后的 palette_version 位置） |

## 构建与运行

```bash
cd hawk-server-rs
cargo build --release          # 产物 target/release/hawk-server(.exe)
cargo test                     # 单元测试（纯函数：路径/颜色/TOML/ignore 匹配/BLAKE3 向量/调色板）
```

运行协议与 C# 版完全一致：

```bash
HAWK_TOKEN=<token> hawk-server --library <素材库路径> --port 27371 [--web-dist <dir>]
```

## 测试

```bash
# 端到端冒烟（仓库根 tools/smoke.sh，语言无关的契约测试；默认测 Rust 版）
bash tools/smoke.sh           # 测 Rust 版（需先 cargo build --release）
bash tools/smoke.sh csharp    # 测 C# 版（dotnet 运行 Debug 产物，保留期内回归用）
```

### 性能压测（tools/bench-*.py，改代码前后各跑一次对比 RESULT 行）

所有脚本输出带 git SHA 的 JSON（`RESULT {...}` 行），服务发现取最新的构建产物
（`target/release` 与 `target/<triple>/release` 并存时按修改时间选择）：

```bash
python3 tools/bench-startup.py --items 5000     # 启动：热启动就绪 / 冷启动（TOML 回退）/ RSS
python3 tools/bench-ingest.py --files 2000      # 入库：watcher 批量入库吞吐、洪峰期读 p99、reindex 全量 vs 复用
python3 tools/bench-images.py --count 500       # 图像管线（默认采样 D:/Materials 真实照片）：
                                                #   img/s、每图 CPU 毫秒、webp 压缩率、palette 覆盖率、丢失数
python3 tools/bench-scale.py --items 30000      # 大库全链路：启动→索引→缩略图→查询延迟（排序变体/颜色检索）
```

参考基线（本机 16 线程 / NVMe / 提交 f0be5af）：

| 指标 | 基线值 |
| ---- | ---- |
| 热启动就绪（3k item） | 0.01s / RSS 27MB |
| 批量入库（1.5k 小图，watcher 路径） | 73 files/s，洪峰期读 p99 29ms |
| 缩略图管线（400 张真实照片 650MB） | 40 img/s，314 CPU-ms/图，webp 占比 0.6%/1.9%/6.1%（256/512/1024），palette 97%（12 张源图不可解码） |
| 大库查询（12k item） | list 1.9ms / skeleton 15.5ms / folder 8.4ms / 颜色检索 ~ms 级 |
| 大库全链路（12k 小图冷建） | 94s，RSS 峰值 91MB，thumbs_missing 0 |

## 在 app 中使用

Electron 主进程的开发态直接运行 `hawk-server-rs/target/release/hawk-server(.exe)`
（release 缺失时回退 debug 构建）；`HAWK_SERVER_EXE` 环境变量可指向任意二进制覆盖。
打包与 CI 经 `scripts/build-server.mjs`（`cargo build --release`，RID 别名映射 rust target）。

## OpenAPI schema

`/openapi/v1.json` 由 `hawk-server-rs/openapi.json` 静态文件服务（`include_str!` 固化）。
schema 即契约（前端类型从它生成），内容固化自 C# 版输出、不随后端实现漂移。
若 API 变更：先改 C# 版（或在 schema 上直接编辑），重新生成该文件：

```bash
# 从运行中的 C# 版抽取（Rust 版挂载了同一路径，运行中的任一边均可）
curl -s http://127.0.0.1:<port>/openapi/v1.json | python -c "import json,sys; d=json.load(sys.stdin); d['servers']=[{'url':'http://127.0.0.1:27371/'}]; print(json.dumps(d, indent=2, ensure_ascii=False))" > hawk-server-rs/openapi.json
```

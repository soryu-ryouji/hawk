# hawk-daemon（Rust 实现）

> 逐文件的代码职责见 [代码导读](server-code-structure.md)。
> API 契约见 [REST API V1](server-rest-api-v1.md)，存储格式见 [storage.md](storage.md)。

hawk 素材管理后端，Rust 实现（`hawk-daemon/`）。
行为对齐基准 = OpenAPI schema + `.hawk/` 存储格式 + SSE 事件契约。

## 状态

- **app 唯一后端**：`hawk-app` 的开发态（`resolveServerCommand`）、打包（`scripts/build-server.mjs`）、
  CI（`release.yml`）全部使用本实现
- **已实现**：全部 REST API、SSE、文件监听、索引流水线（防抖/扫描 runner 线程：并行哈希+单次解码产出派生+结果回流消费循环穿插应用/对账扫描）、缩略图（libwebp q80，导入即生成+读取端兜底）、
  调色板（median-cut）、SQLite 派生缓存（schema v1）
- **设计与实现要点**：
  - 宽高在入库时即持久化入 TOML（与 storage.md 设计意图一致，重启无需靠扫描重新识别）
  - 调色板算法为 median-cut，同 hash 结果确定性一致（按占比降序、并列按 RGB 升序）
  - 并发度：哈希并行取物理核估计（逻辑核/2，封顶 16；解码/编码 SIMD 密集，SMT 无增益，兄弟线程留给消费循环/API），缩略图 worker `CPU/2` 封顶 12——桌面端大批量导入/重建索引时吞吐优先
  - 查询路径不克隆完整 DTO：排序在轻量键上进行，`item/list` 只投影分页窗口、`item/skeleton` 只投影轻量骨架（大库下 skeleton 持锁时间降低一个量级，UI 重连重同步不再卡死读路径）
  - 调色板回写按批冲刷（≥500 条或滞留 2s）：TOML 逐条落盘、SQLite 单事务、事件按批平滑补发——全库重提炼（缓存重建）时无 `item.updated` 洪峰
  - 缩略图 worker 无界队列 + 周期对账自愈：队列无界（任务 ~100B、in-flight 去重后内存有界），周期对账扫描 palette 缺失项与宽高为 0 的项并派发补齐任务（缩略图不在对账中批量生成；宽高另有读取端自愈：`item/list`/`item/skeleton` 响应中发现 0 宽高即派发）
  - **缩略图为惰性缓存**：入库/启动对账只生成调色板（颜色搜索依赖全量）；缩略图由读取端触发——`/item/thumbnail` 未命中时直接回源原图（浏览器可渲染格式）并后台入队生成，命中后返回 webp。首次查看零等待，未查看的素材零成本
  - 启动注水经 TOML 全量回退时上报进度（每 1000 文件一帧，phase=sync），大库首次启动启动屏有实时反馈

## 技术选型

| 职责 | 选型 | 备注 |
| ---- | ---- | ---- |
| HTTP 框架 | axum 0.8 | tower 中间件链 |
| 异步运行时 | tokio | pipeline 消费循环与扫描 runner 均为专用 OS 线程（阻塞扫描不占运行时线程，也不阻塞消费循环） |
| 文件监听 | notify 8 | From/To rename 配对 + 300ms 超时兜底 |
| 哈希 | blake3 | item id = BLAKE3 hex（存储契约） |
| 图像解码/缩放 | image 0.25 + fast_image_resize | Lanczos3 |
| WebP 编码 | webp（libwebp） | 有损 q80；纯 Rust 的 image-webp 仅支持无损 |
| 元数据缓存 | rusqlite（bundled） | schema v1，版本不符整库重建 |
| TOML | toml（解析）+ 手写序列化 | 输出格式精确可控（标量在前、`[[paths]]`/`[[palette]]` 在后） |

## 构建与运行

```bash
cd hawk-daemon
cargo build --release          # 产物 target/release/hawk-daemon(.exe)
cargo test                     # 单元测试（纯函数：路径/颜色/TOML/ignore 匹配/BLAKE3 向量/调色板）
```

运行协议：

```bash
HAWK_TOKEN=<token> hawk-daemon --library <素材库路径> --port 27371 [--web-dist <dir>]
```

## 测试

```bash
# 端到端冒烟（仓库根 tools/smoke.sh，契约测试；需先 cargo build --release）
bash tools/smoke.sh
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

参考基线一（参考机 16 线程 / NVMe / 提交 f0be5af）：

| 指标 | 基线值 |
| ---- | ---- |
| 热启动就绪（3k item） | 0.01s / RSS 27MB |
| 批量入库（1.5k 小图，watcher 路径） | 73 files/s，洪峰期读 p99 29ms |
| 缩略图管线（400 张真实照片 650MB，全量生成语义的旧数据，现行为两阶段：调色板即时 + 缩略图读取端触发） | 40 img/s，314 CPU-ms/图，webp 占比 0.6%/1.9%/6.1%（256/512/1024），palette 97%（12 张源图不可解码） |
| 大库查询（12k item） | list 1.9ms / skeleton 15.5ms / folder 8.4ms / 颜色检索 ~ms 级 |
| 大库全链路（12k 小图冷建） | 94s，RSS 峰值 91MB，thumbs_missing 0 |

参考基线二（开发机 24 线程（12 物理核）/ NVMe / 流水线重构后 + 落盘批量化 + 物理核并行度，
同机前后对照见行末括号）：

| 指标 | 基线值（vs 落盘批量化前） |
| ---- | ---- |
| 热启动就绪（5k item） | 0.01s；冷缓存重建扫描 0.6s / RSS 26MB |
| 批量入库（2k 小图，watcher 路径） | **351 files/s**（前 75.7，4.6x），洪峰期读 p99 26ms，RSS 21MB |
| reindex 全量 vs 复用（2k） | 2.2s vs 0.3s |
| 图像管线（500 张真实照片 809MB） | 22.4 img/s（持平），260 CPU-ms/图（前 464，SMT 争用消除 -44%），webp 占比 0.6%/2%/6.4%，palette 100%，解码失败 0 |
| 大库全链路（12k 小图冷建） | **75s / 160 item/s**（前 226s / 53 item/s，3.0x），RSS 峰值 46MB，thumbs_missing 0 |
| 大库全链路（2k 小图冷建） | 33.2s（前 53.3s，1.6x） |
| 大库查询（12k item，洪峰期） | list 16ms / skeleton 15ms / folder 11ms / 颜色 16ms；count p50 2.0ms（前 15.8ms） |

吞吐瓶颈定位：小图路径原瓶颈是消费循环串行 apply 与 SMT 争用，落盘批量化（SQLite 单事务摊薄）
+ 物理核并行度后已不再是第一瓶颈；真实照片路径是编码 CPU 密集（三尺寸 WebP q80 + 调色板），
≈0.26 CPU-s/张，处于参数上限。哈希本身（BLAKE3）从不构成瓶颈。

## 在 app 中使用

Electron 主进程的开发态直接运行 `hawk-daemon/target/release/hawk-daemon(.exe)`
（release 缺失时回退 debug 构建）；`HAWK_DAEMON_EXE` 环境变量可指向任意二进制覆盖。
打包与 CI 经 `scripts/build-server.mjs`（`cargo build --release`，RID 别名映射 rust target）。

## OpenAPI schema

`/openapi/v1.json` 由 `hawk-daemon/openapi.json` 静态文件服务（`include_str!` 固化）。
schema 即契约（前端类型从它生成），不随后端实现漂移。
若 API 变更：直接编辑 schema 文件，或改完后经运行中的服务回写：

```bash
# 从运行中的服务抽取（servers 段归一到默认端口）
curl -s http://127.0.0.1:<port>/openapi/v1.json | python -c "import json,sys; d=json.load(sys.stdin); d['servers']=[{'url':'http://127.0.0.1:27371/'}]; print(json.dumps(d, indent=2, ensure_ascii=False))" > hawk-daemon/openapi.json
```

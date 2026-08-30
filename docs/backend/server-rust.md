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

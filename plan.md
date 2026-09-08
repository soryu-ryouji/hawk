# 架构整改计划（临时文件，完成后删除）

目标：落实架构评估中的全部改进项。逐项提交，每项完成即更新本文件。

## 状态

| # | 项 | 状态 | 提交 |
| - | -- | ---- | ---- |
| 1 | P0-3 静态检查门禁：cargo fmt/clippy + 前端 ESLint/Prettier + CI | DONE | 见 git log |
| 2 | P2-11 panic 策略：移除 release panic="abort"（恢复 catch_unwind 隔离）+ 请求/worker panic 兜底 | DONE | 071e58d |
| 3 | P1-4 hawk-daemon 拆 lib + 薄 bin | DONE | 见 git log |
| 4 | P0-2 配置解析容错：保留上次有效配置 + 错误暴露 | DONE | 见 git log |
| 5 | P1-5 可观测性：app/status 扩展（扫描统计/队列溢出/配置错误/SSE lag） | DONE | 见 git log |
| 6 | P0-1 周期 FS 对账兜底（HAWK_FS_RESCAN_INTERVAL，默认 900s） | DONE | 见 git log |
| 7 | P1-7 API 策略拒绝返回 4xx（add/upload/rename 前置校验） | DONE | 见 git log |
| 8 | P1-6 文档机制化：API 文档-代码契约测试 + 文档链接检查 | DONE | 见 git log |
| 9 | P2-9 前端全局错误处理（errorHandler + unhandledrejection） | DONE | 见 git log |
| 10 | P2-8 前端 library store 拆分 | TODO | |
| 11 | P2-12 依赖审计：dependabot + cargo audit + npm audit | TODO | |
| 12 | P2-10 跨仓协议契约 | SKIP（hawk-remote 未实现，无被测对象；落地时补） | - |

## 约定

- 每项完成后：`cargo test`（daemon）+ `npm run test:unit`（前端）+ 相关契约产物重新固化
- 提交信息中文，前缀英文；`feat`/`fix` 触发 nightly
- 本文件完成后删除

## 进度记录

（每完成一项在此追加一行：日期 + 项 + 验证方式）

- 2026-09-08 #1 完成：cargo fmt/clippy 全绿；前端 eslint+prettier 全绿；CI rust 加 fmt/clippy、desktop 加 lint/format:check
- 2026-09-08 #2 完成：移除 panic=abort，worker/CatchPanic/panic hook 隔离齐备
- 2026-09-08 #3 完成：lib.rs + 薄 bin + tests/openapi_contract.rs；clippy 因可见性暴露的 4 处收敛为 pub(crate)
- 2026-09-08 #4+#5 完成：config load→Result、last_error 暴露；app/status 增 last_scan/queue_overflow/config_error/sse_lagged；契约产物重固化
- 2026-09-08 #6 完成：HAWK_FS_RESCAN_INTERVAL（默认 900s，force_walk 兜底）；HAWK_RESCAN_INTERVAL 更名 HAWK_RECONCILE_INTERVAL；e2e 验证 ticker 与 status
- 2026-09-08 #7 完成：add/upload/update 前置校验 ignore+白名单，4xx 明确报错
- 2026-09-08 #8 完成：contract_tests 增文档-代码契约（端点+SSE 双向）；tools/check-docs.py + CI 链接检查（50 条）
- 2026-09-08 #9 完成：main.ts 全局错误兜底（errorHandler/unhandledrejection/error + 5s 去重 toast）

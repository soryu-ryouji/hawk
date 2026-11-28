# 说明

- 不以维护向后兼容性为目标。对于已经废弃的代码路径，应直接移除，不再通过兼容层、回退机制或迁移方案予以保留。
- 在充分满足当前需求的前提下，采用尽可能简单的实现方案。避免引入缺乏实际需求依据的抽象、配置项和间接层。
- 采用渐进式、分层的方式构建系统。首先完成能够端到端运行的最小版本，再基于稳定可用的产品逐步增加功能。不要以尚未成熟的复杂性取代已经可用的产品。
- 保持组件的模块化，并明确划分不同职责与关注点。
- 当成熟且维护良好的库能够降低整体复杂度或提高可靠性时，应优先采用。除非有明确理由，不要重复实现通用功能。
- 在自行实现功能或新增依赖之前，应优先评估项目现有依赖的能力。应先查阅相关文档和类型定义，不应未经确认就认定某个库不具备所需能力。
- 架构决策应着眼于长期演进。不要采用仅能解决当前问题、且预期需要在后续替换的权宜方案。
- 在设计解决方案之前，先研究成熟产品如何解决同类问题。优先采用经过验证的模式和约定，避免从零开始另行设计一套方案。

- 更新了功能之后，需要同步更新对应的文档

- API 契约的单一来源是 `hawk-daemon/openapi.json`（Go handler 与 DTO 按它实现），任何改动后必须重新固化两份产物，缺一不可：
  1. 编辑 `hawk-daemon/openapi.json`
  2. `node tools/sync-openapi.mjs`（同步到 `internal/api/openapi.json`，`go:embed` 使用）
  3. `cd hawk-daemon && go test ./...`（`TestEmbeddedOpenAPIMatchesRepository` 校验同步）
  4. `cd hawk-app && npm run gen:types`（拉起 Go daemon 读取 `/openapi/v1.json` 生成 `schema.d.ts`）
  守护：Go 契约测试与 CI 重跑 gen:types + diff 检查。漏跑任一步即测试/CI 失败。

## 提交前自检（本地对齐 CI 门禁）

生成或修改代码后，提交前必须先在本地跑完改动范围对应的检查、全绿再提交。命令与 `.github/workflows/ci.yml` 的质量门禁逐条对应，本地漏跑任何一条，CI 检查 job 即失败，整条构建/发布链被跳过：

- 改动 `hawk-daemon`（Go，在 `hawk-daemon` 目录执行）：
  1. `gofmt -w .`（写入式格式化）
  2. `go vet ./...`
  3. `go test ./... -count=1`
  4. 需要二进制时 `go build -o hawk-daemon.exe .`
- 改动 `hawk-app`（web / electron）：
  1. `npm run format`（写入式格式化）
  2. `npm run lint`
  3. `npm run build`（vue-tsc + 主进程 tsc 类型检查 + vite 构建）
  4. `npm run test:unit`
- 改动 `hawk-ios`（iOS 原生客户端）：
  1. `xcrun swift-format format --in-place --recursive hawk-ios`（写入式格式化，配置在 `hawk-ios/.swift-format`）
  2. `xcrun swift-format lint --strict --recursive hawk-ios`
  3. `xcodebuild -scheme hawk-ios -destination 'generic/platform=iOS Simulator' build`
  4. `xcodebuild test -scheme hawk-ios -destination 'platform=iOS Simulator,name=iPhone 17'`（测试 target 就位后）
  5. 涉及连接/网络时补真机冒烟：局域网直连（二维码 + 地址密钥各一次；模拟器无法验证本地网络权限与后台恢复）
  iOS 构建暂不进 CI（需 macOS runner 与签名），但本地必须全绿再提交
- 改动 `hawk-android`（Android 原生客户端，在 `hawk-android` 目录执行，`JAVA_HOME` 指向 Android Studio 自带 JBR）：
  1. `./gradlew :app:assembleDebug`
  2. `./gradlew :app:lintDebug`（0 error）
  3. `./gradlew :app:testDebugUnitTest`
  4. 涉及连接/网络时补模拟器或真机冒烟：局域网直连（二维码 + 地址密钥各一次）
  Android 构建暂不进 CI（需 Android SDK 与签名），但本地必须全绿再提交
- 改动 `README.md` / `docs/` / `AGENTS.md`：仓库根 `python3 tools/check-docs.py`（相对链接检查）
- 涉及 API 契约（含只改注释）：另执行上文三段式生成链，重新固化两份产物

打包冒烟与 UI 端到端（`pack:dir`、ui-check）本地不强制；若改动打包配置或主进程/daemon 启动链路，提交前建议先跑 `npm run pack:dir` 验证。

当前项目没有正式上线，你不需要考虑版本兼容性和提版本号，如果设计上需要版本号，则使用最小版本号（不需要考虑测试环境的版本兼容）
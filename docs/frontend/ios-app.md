# hawk iOS 客户端设计

> 当前实现为纯局域网直连：客户端通过 daemon 的 peer 端口（默认 27375）用 AccessKey 直接访问 `/api/v1/*`。

iPhone / iPad 原生客户端使用 SwiftUI 与 Swift 6。本文记录当前协议、模块边界、已实现能力与后续路线；接口字段以 [REST API V1](../backend/server-rest-api-v1.md)、`hawk-daemon/openapi.json` 和 daemon DTO 为单一来源。

## 目标与边界

当前目标：

- 在 iPhone / iPad 上原生浏览、搜索和筛选自己的素材库。
- 同一局域网内使用 AccessKey 直连 daemon HTTP，图片、查询与 SSE 走同一通道。
- 支持锁视图、排序偏好、原图保存/系统分享和相册导入。
- 后续再增加本地索引、离线浏览、Spotlight / App Intents 与端上 AI。

当前不替代桌面管理端，不实现素材库设置、扫库、回收站清空和通用编辑写回。Android 独立实现，见 [Android 客户端设计](android-app.md)；不提前抽象跨平台 UI 层。

## 工程约定

| 项 | 决策 |
| -- | -- |
| 部署目标 | iOS / iPadOS 26.0，iPhone + iPad 单 target |
| UI | SwiftUI；预览缩放使用 UIKit `UIScrollView` 桥接 |
| 并发 | Swift 6，`SWIFT_DEFAULT_ACTOR_ISOLATION = MainActor`，网络类型显式 `nonisolated` / `Sendable` |
| 工程 | `hawk-ios.xcodeproj`、共享 scheme、同步文件夹；新增 Swift 文件无需逐个登记 |
| 测试 | `hawk-iosTests` XCTest target，覆盖配对与网络契约 |
| 原生依赖 | 无第三方原生依赖；纯 `URLSession` HTTP 客户端，GRDB 仅在实现离线索引时引入 |
| 格式化 | `xcrun swift-format`，配置为 `hawk-ios/.swift-format` |

## 连接与凭证

### V3 配对载荷

桌面端生成 iOS / Android 共用 JSON：

```json
{
  "v": 3,
  "target_library_id": "library-id",
  "access_key": "hk_...",
  "lan_endpoints": ["http://192.168.1.5:27375"]
}
```

约束：

- `access_key` 与至少一个 `lan_endpoints` 必填；`target_library_id` 可选。
- 地址只接受 `http`；未写 scheme 时按 `http` 补全。AccessKey 是长期凭证，存 Keychain。
- 依次尝试 `lan_endpoints`，首个作为连接的展示地址；连接前用公开的 `app/lan/connection-info` 校验 `protocol=3`。
- 连接以地址 + AccessKey 定位，保存时按地址去重。
- 不支持其他 `v`，不向旧 V1/V2 载荷降级。

### 数据通路

| 形态 | 通路 | 鉴权 |
| -- | -- | -- |
| 局域网直连 | App → daemon peer 端口 `http://<IP>:27375` 的 `/api/v1/*` | `Authorization: Bearer <access_key>` |

连接流程：

1. 对每个候选地址读取 `GET /api/v1/app/lan/connection-info`，要求 `protocol=3`。
2. 用选中地址与 AccessKey 创建 `APIClient`，`GET /api/v1/app/info` 校验 `has_library`。
3. `GET /api/v1/app/startup` 轮询到 `ready`，再进入素材库首屏。

断开连接只做本地清理（取消任务、停止 SSE、清空分享目标），不发起额外的会话请求。

### 请求鉴权

- JSON REST、缩略图、原图下载和 SSE 全部使用 `Authorization: Bearer <access_key>`。
- AccessKey 不进入媒体或 SSE URL。
- 401 不做全局自动重认证：凭证失效与解锁密码错误必须保留各自的服务端错误语义。
- `X-Hawk-Unlock` 用于 JSON 请求；媒体与 SSE 只通过 `?unlock=` 传解锁票据。

## 首屏与浏览状态

在线首屏不建本地索引：

1. `app/info` 验证角色与库状态，`app/startup` 等待索引就绪。
2. 并发读取 `library/info`、`global_filter/list`、`lock/list` 和 `view/preferences`，完成后立即进入主界面。
3. 首页 `item/list`（48 条）与完整 `item/skeleton` 并发；首页详情先渲染，骨架到达后接管虚拟网格。
4. `folder/list`、`category/list`、`tag/list` 与五个智能视图计数后台补齐。

模块职责：

- `APIClient`：信封、Bearer、解锁票据、媒体与 SSE。
- `ConnectionLauncher`：候选地址、连接信息校验与启动等待。
- `LibraryContext`：会话级库信息、权限、分类、锁、隐藏集、计数、排序偏好和实时事件。
- `GridStore`：单视图的查询、骨架、详情窗口、分页和锁定态。
- `ImageStore`：Bearer 图片请求、降采样、内存/磁盘缓存和相邻页预取。

手工下拉刷新与 SSE 重连共用全量对齐：重新读取权限、隐藏集、锁、分类、计数和排序偏好，并推进网格结构版本。

## 查询与界面

- iPhone 使用素材/浏览 Tab；iPad 使用 `NavigationSplitView`，窄窗口退化为 Tab。
- 网格使用骨架和齐行布局，按视口加载 48 条详情窗口；同内容多位置以 `id + path` 区分。
- 支持名称/备注关键词、评分、短边、颜色、八种排序和文件夹/分类/标签/回收站视图。
- `ScanInfo.interval` 单位是**秒**，不是分钟。
- 只有 editor/admin 可写 `view/preference`；viewer、importer 不发起写请求。
- 全屏预览支持翻页、双击/双指缩放、平移、下拉关闭、保存原图、系统分享和素材信息。

`ItemListRequest` 与 daemon 共用 snake_case 字段，包括 `categories_match`、尺寸区间和隐藏集排除字段。解码允许新增可选字段，但不保留已删除的 gate/login/viewer DTO。

## SSE 实时同步

`/api/v1/events` 使用 Bearer 长连接，按 1s→15s 退避重连；后台停止，前台恢复。

| 事件 | 客户端行为 |
| -- | -- |
| `item.updated` | 更新详情与媒体版本；单条变更可能改变路径、成员或排序，因此重查骨架并安排分类/计数刷新 |
| `items.updated` | 批量更新详情与媒体版本；标签/分类删除等通用批量变更也走此事件，因此推进结构版本，由 GridStore 防抖重查并刷新分类/计数 |
| `item.added` / `items.added` / `item.removed` / `item.trashed` / `item.restored` | 推进结构版本并刷新分类与计数 |
| `folder.changed` / `locks.changed` / `global_filter.changed` | 应用快照或重查分类，并推进结构版本 |
| `library.updated` | 更新库信息 |
| `task.progress` | 更新后台任务提示 |
| `lan.access.changed` | 重新请求 `app/info`，刷新角色与写权限 |

每个条目还有媒体版本：缩略图尚未生成时的 404 不会被当作永久失败；对应 `item.updated` 到达后，网格和预览会重新加载相同 URL。媒体版本同时进入内存/in-flight cache key，非零版本绕过旧 URLSession 磁盘缓存，因此强制重建后不会继续显示旧图。

## 分享扩展

Share Extension 与主 App 通过 App Group `group.com.soryu-ryouji.hawk` 共享 `shareTarget.v3`：候选地址、AccessKey 和展示名。

- 扩展按候选地址与 AccessKey 直连 daemon HTTP API，不依赖主 App 常驻。
- 连接时先请求 `library/info` 确认目标可用，再读取分类/标签。
- 上传：`POST /api/v1/item/upload`，multipart 中重复 `categories` / `tags`，`skip_existing=true`，目标为库根。
- 同内容已存在：响应 `skipped=true`，计为跳过。
- 同名不同内容：daemon 返回 `FILE_EXISTS`，明确失败，不能静默算作重复成功。
- 最多 20 张，逐张读取与串行上传；JPEG/PNG/GIF/WebP 原样上传，HEIC/HEIF 转 PNG。
- 关闭扩展时清理临时文件后调用 `completeRequest`。
- viewer 无导入权限；importer/editor/admin 的实际能力仍由 daemon 的角色与导入权限决定。

## 本地检查

在 `hawk-ios/` 执行：

```bash
xcrun swift-format format --in-place --recursive hawk-ios
xcrun swift-format lint --strict --recursive hawk-ios
xcodebuild -scheme hawk-ios -destination 'generic/platform=iOS Simulator' build
xcodebuild test -scheme hawk-ios -destination 'platform=iOS Simulator,name=iPhone 17'
```

涉及连接/网络时还需真机冒烟：局域网扫码与手填、候选地址回退、分享扩展、锁定视图、权限降级、后台恢复各跑通一次。模拟器不能替代本地网络权限与后台恢复验证。

## 后续路线

| 阶段 | 内容 | 状态 |
| -- | -- | -- |
| M0/M1 | 工程、V3 局域网连接、浏览、筛选、预览、锁、SSE、原图保存、分享导入、协议测试 | 已完成核心链路；真机网络矩阵持续验收 |
| M2 | GRDB 本地索引、首屏快照、离线浏览、App Intents / Spotlight | 未开始；SSE 已提前落地 |
| M3 | Foundation Models 自然语言查询、Vision / 图片理解 | 未开始；只生成结构化查询，不直接拼 URL |
| M4 | 按明确安全边界评估标签/备注写回 | 未开始 |

客户端只支持局域网直连；离线索引、AI 和写回不得反向扩大现有 AccessKey 权限边界。

## 相关文档

- [架构设计](../architecture.md)
- [REST API V1](../backend/server-rest-api-v1.md)
- [权限与访问控制](../backend/access-control.md)
- [Android 客户端设计](android-app.md)

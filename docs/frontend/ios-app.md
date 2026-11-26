# hawk iOS 客户端设计

> 当前实现以 [权限与传输架构 V2](../backend/access-control-v2.md) 为准。公网网页版、`/viewer/*`、tenant-key 查看登录和 URL token 已删除；原生 LAN/WAN 均通过 `hawk-transport` 建立 iroh 端到端加密会话。

iPhone / iPad 原生客户端使用 SwiftUI 与 Swift 6。本文记录当前协议、模块边界、已实现能力与后续路线；接口字段以 [REST API V1](../backend/server-rest-api-v1.md)、`hawk-daemon/openapi.json` 和 daemon DTO 为单一来源。

## 目标与边界

当前目标：

- 在 iPhone / iPad 上原生浏览、搜索和筛选自己的素材库。
- LAN/WAN 使用同一 AccessKey 与目标身份，图片、查询和 SSE 均经加密通道。
- 支持锁视图、排序偏好、原图保存/系统分享和相册导入。
- 后续再增加本地索引、离线浏览、Spotlight / App Intents 与端上 AI。

当前不替代桌面管理端，不实现素材库设置、扫库、回收站清空和通用编辑写回。Android 独立实现，见 [Android 客户端设计](android-app.md)；不提前抽象跨平台 UI 层。

## 工程约定

| 项 | 决策 |
| -- | -- |
| 部署目标 | iOS / iPadOS 26.0，iPhone + iPad 单 target |
| UI | SwiftUI；预览缩放使用 UIKit `UIScrollView` 桥接 |
| 并发 | Swift 6，`SWIFT_DEFAULT_ACTOR_ISOLATION = MainActor`，网络与原生传输类型显式 `nonisolated` / `Sendable` |
| 工程 | `hawk-ios.xcodeproj`、共享 scheme、同步文件夹；新增 Swift 文件无需逐个登记 |
| 测试 | `hawk-iosTests` XCTest target，覆盖配对与网络契约 |
| 原生库 | `tools/build-mobile-transport.sh ios` 生成静态 `HawkTransport.xcframework`，主 App 与 Share Extension 分别链接 |
| 本地依赖 | Rust 工具链与 iOS Rust targets；GRDB 仅在未来实现离线索引时引入 |
| 格式化 | `xcrun swift-format`，配置为 `hawk-ios/.swift-format` |

`hawk-ios/Frameworks/` 是可重建产物，不入库。`libc` 在 `hawk-transport` 中暂锁定 `0.2.189`：后续版本把 iroh 的 `netdev/netwatch` 所需 XNU 声明限制为 macOS，会导致 iOS 编译失败。原生库改动后必须重新生成 XCFramework。

## 连接与凭证

### V2 配对载荷

桌面端生成 iOS / Android 共用 JSON：

```json
{
  "v": 2,
  "target_library_id": "library-id",
  "target_device_id": "device-id",
  "target_identity": "iroh-endpoint-public-key",
  "tls_fingerprint": "optional-lan-web-fingerprint",
  "access_key": "hk_...",
  "lan_endpoints": ["http://192.168.1.5:27375"],
  "wan": { "server_url": "https://hawk.example.com" }
}
```

约束：

- `target_identity` 与 `access_key` 必填；WAN 还必须有 `target_device_id`。
- `target_identity` 用于带外钉扎目标公钥，不是秘密；AccessKey 是长期凭证，存 Keychain。
- WAN 服务地址优先，失败后可尝试 LAN 候选地址。原生通道由 daemon 返回的 `app/info.channel` 判定，UI 不仅凭 profile 类型猜测。
- 同一 hawk-server 可对应多个目标，保存连接时按 `targetIdentity + targetLibraryId` 去重，而不是只按服务器地址。
- 不支持其他 `v`，不向旧 V1 载荷降级。

### 数据通路

| 形态 | 通路 | 鉴权 |
| -- | -- | -- |
| 原生 LAN | rendezvous → iroh → daemon；UI → 本机 Loopback 代理 | 隧道内验证 AccessKey；Loopback 请求使用代理签发的短期 Bearer `session_token` |
| 原生 WAN | hawk-server 编排 rendezvous → iroh 直连或 relay → daemon | 同上；server 不接触 AccessKey 或素材明文 |
| 手工 LAN | App → LAN Web API | AccessKey Bearer；仅作为无配对信息时的兜底 |

原生库返回 `{ session_id, api_base, credential, role, target_device_id }` 或 `{ error }`。Swift 必须先处理错误响应，再要求成功字段完整。

`NativeTransportSession` 持有 `session_id`：切换、断开、初始化失败及扩展结束时调用 `hawk_transport_disconnect`，不能调用已删除的 HTTP `/viewer/session`。断开操作在后台线程执行，避免阻塞 UI。

### 请求鉴权

- JSON REST、缩略图、原图下载和 SSE 全部使用 `Authorization: Bearer ...`。
- AccessKey 和代理 `session_token` 不进入媒体或 SSE URL。
- LAN Web 的 `hawk_session` Cookie 属于浏览器 `/app/session` 流程，原生客户端不伪造该 Cookie。
- 401 不做全局自动重认证：凭证失效与解锁密码错误必须保留各自的服务端错误语义。
- `X-Hawk-Unlock` 用于 JSON 请求；媒体与 SSE 只通过 `?unlock=` 传解锁票据。

## 首屏与浏览状态

在线首屏不建本地索引：

1. `app/info` 验证角色、通道与库状态，`app/startup` 等待索引就绪。
2. 并发读取 `library/info`、`global_filter/list`、`lock/list` 和 `view/preferences`，完成后立即进入主界面。
3. 首页 `item/list`（48 条）与完整 `item/skeleton` 并发；首页详情先渲染，骨架到达后接管虚拟网格。
4. `folder/list`、`category/list`、`tag/list` 与五个智能视图计数后台补齐。

模块职责：

- `APIClient`：信封、Bearer、解锁票据、媒体与 SSE。
- `ConnectionLauncher` / `RustTransport`：候选连接、原生会话与启动等待。
- `LibraryContext`：会话级库信息、权限、分类、锁、隐藏集、计数、排序偏好和实时事件。
- `GridStore`：单视图的查询、骨架、详情窗口、分页和锁定态。
- `ImageStore`：Bearer 图片请求、降采样、内存/磁盘缓存和相邻页预取。

手工下拉刷新与 SSE 重连共用全量对齐：重新读取权限、隐藏集、锁、分类、计数和排序偏好，并推进网格结构版本。

## 查询与界面

- iPhone 使用素材/浏览 Tab；iPad 使用 `NavigationSplitView`，窄窗口退化为 Tab。
- 网格使用骨架和齐行布局，按视口加载 48 条详情窗口；同内容多位置以 `id + path` 区分。
- 支持名称/备注关键词、评分、短边、颜色、八种排序和文件夹/分类/标签/回收站视图。
- `ScanInfo.interval` 单位是**秒**，不是分钟。
- 只有 LAN 的 editor/admin 可写 `view/preference`；viewer、importer 和 WAN 通道不发起写请求。
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
| `lan.access.changed` | 重新请求 `app/info`，刷新角色、通道和写权限 |

每个条目还有媒体版本：缩略图尚未生成时的 404 不会被当作永久失败；对应 `item.updated` 到达后，网格和预览会重新加载相同 URL。媒体版本同时进入内存/in-flight cache key，非零版本绕过旧 URLSession 磁盘缓存，因此强制重建后不会继续显示旧图。

## 分享扩展

Share Extension 与主 App 通过 App Group `group.com.soryu-ryouji.hawk` 共享 `shareTarget.v2`：候选地址、AccessKey、目标身份、目标设备 ID 和展示名。

- 原生 LAN/WAN：扩展独立调用 `hawk-transport`，不能直接把 Peer 地址当素材 REST 地址，也不依赖主 App 的 Loopback 进程存活。
- 手工 LAN：可直接访问 LAN Web。
- 上传：`POST /api/v1/item/upload`，multipart 中重复 `categories` / `tags`，`skip_existing=true`，目标为库根。
- 同内容已存在：响应 `skipped=true`，计为跳过。
- 同名不同内容：daemon 返回 `FILE_EXISTS`，明确失败，不能静默算作重复成功。
- 最多 20 张，逐张读取与串行上传；JPEG/PNG/GIF/WebP 原样上传，HEIC/HEIF 转 PNG。
- 关闭扩展时等待原生断开完成（控制面请求 5 秒超时）后再调用 `completeRequest`，避免进程回收导致会话只能等租约过期。
- viewer 无导入权限；importer/editor/admin 的实际能力仍由 daemon 的角色、通道和 WAN 导入开关共同决定。

## 本地检查

在仓库根构建原生库：

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
tools/build-mobile-transport.sh ios
```

在 `hawk-ios/` 执行：

```bash
xcrun swift-format format --in-place --recursive hawk-ios
xcrun swift-format lint --strict --recursive hawk-ios
xcodebuild -scheme hawk-ios -destination 'generic/platform=iOS Simulator' build
xcodebuild test -scheme hawk-ios -destination 'platform=iOS Simulator,name=iPhone 17'
```

涉及连接/网络时还需真机冒烟：原生 LAN、WAN、手工 LAN、分享扩展、锁定视图、权限降级、后台恢复各跑通一次。模拟器不能替代本地网络权限与后台恢复验证。

## 后续路线

| 阶段 | 内容 | 状态 |
| -- | -- | -- |
| M0/M1 | 工程、V2 连接、浏览、筛选、预览、锁、SSE、原图保存、分享导入、协议测试 | 已完成核心链路；真机网络矩阵持续验收 |
| M2 | GRDB 本地索引、首屏快照、离线浏览、App Intents / Spotlight | 未开始；SSE 已提前落地 |
| M3 | Foundation Models 自然语言查询、Vision / 图片理解 | 未开始；只生成结构化查询，不直接拼 URL |
| M4 | 按明确安全边界评估标签/备注写回 | 未开始 |

iroh 已是当前原生传输，不再列为未来直连里程碑。离线索引、AI 和写回不得反向扩大现有 AccessKey 或 WAN 权限边界。

## 相关文档

- [架构设计](../architecture.md)
- [REST API V1](../backend/server-rest-api-v1.md)
- [权限与传输架构 V2](../backend/access-control-v2.md)
- [远程访问设计](../backend/remote-access.md) / [远程访问协议](../backend/remote-protocol.md)
- [Android 客户端设计](android-app.md)

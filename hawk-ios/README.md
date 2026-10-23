# hawk-ios

iPhone / iPad 原生客户端（SwiftUI）。设计文档见 [../docs/frontend/ios-app.md](../docs/frontend/ios-app.md)。

## 环境

- Xcode 27（iOS 27.1 SDK），部署目标 iOS 26.0
- 真机调试需登录 Apple ID（免费账号签名 7 天有效；TestFlight 分发需付费开发者账号）

## 工程现状

| 项 | 值 |
| -- | -- |
| 工程 / target / scheme | `hawk-ios.xcodeproj` / `hawk-ios` / `hawk-ios`（scheme 已共享，进仓库） |
| 源码目录 | `hawk-ios/hawk-ios/`（`PBXFileSystemSynchronizedRootGroup`：新增/重命名文件直接改磁盘，不用改 `project.pbxproj`） |
| 部署目标 | iOS 26.0 / iPadOS 26.0（Foundation Models 文本能力 26.0，图片理解 27.0，按 `#available` 分支） |
| 设备 | iPhone + iPad 单 target（`TARGETED_DEVICE_FAMILY = "1,2"`） |
| Bundle ID | `com.soryu-ryouji.hawk` |
| Swift | Swift 6 语言模式；`SWIFT_DEFAULT_ACTOR_ISOLATION = MainActor`、`SWIFT_APPROACHABLE_CONCURRENCY = YES` |
| Info.plist | 由构建设置生成（`GENERATE_INFOPLIST_FILE = YES`），键走 `INFOPLIST_KEY_*`，不落文件 |
| 平台 | 仅 `iphoneos iphonesimulator`（不构建 macOS / visionOS） |

新增 Info.plist 键时改 target 的 Build Settings（`INFOPLIST_KEY_<Key>`），不要改成物理 plist 文件。

忽略规则在本目录的 `.gitignore`（与 `hawk-daemon/`、`hawk-app/` 的惯例一致，子项目自管）；仓库根 `.gitignore` 只放跨项目条目。

## 开发与自检

```bash
# 格式化（与 CI 无关，提交前本地跑；配置在 .swift-format）
xcrun swift-format format --in-place --recursive hawk-ios
xcrun swift-format lint --strict --recursive hawk-ios

# 构建（不需要设备与签名）
xcodebuild -scheme hawk-ios -destination 'generic/platform=iOS Simulator' build

# 单测（测试 target 建好后可用）
xcodebuild test -scheme hawk-ios -destination 'platform=iOS Simulator,name=iPhone 17'

# 真机冒烟（模拟器覆盖不到本地网络权限与后台恢复）
# 1) 局域网：桌面端开 [web]，App 手填或扫码导入地址 + token，浏览缩略图与预览
# 2) 广域网：填服务端地址 + service-key + hawk-app key，选在线设备后浏览
```

`NSLocalNetworkUsageDescription` 已配置为「连接同一局域网内的 hawk 素材库」；ATS 保持默认，不开 `NSAllowsArbitraryLoads`。

## 待办

- 单元测试 target（Xcode：`File > New > Target… > Unit Testing Bundle`，target 名 `hawk-iosTests`）
- 目录按里程碑生长，不预铺空目录：`Networking/`（M0）→ `Connection/`、`Library/`（M1）→ `Index/`、`Intents/`（M2）

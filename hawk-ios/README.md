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
| 第三方依赖 | 暂无（GRDB 计划在 M2 引入） |

新增 Info.plist 键时改 target 的 Build Settings（`INFOPLIST_KEY_<Key>`），不要改成物理 plist 文件。当前配置了：

- `NSLocalNetworkUsageDescription`：局域网直连素材库
- `NSCameraUsageDescription`：扫描配对二维码
- `NSPhotoLibraryAddUsageDescription`：保存素材原图到相册（仅添加，不读取相册）
- `UIUserInterfaceStyle = Dark`：与桌面/web 深色主题一致

忽略规则在本目录的 `.gitignore`（与 `hawk-daemon/`、`hawk-app/` 的惯例一致，子项目自管）；仓库根 `.gitignore` 只放跨项目条目。

## 已实现（M1 只读浏览）

- 连接管理
  - 局域网直连：地址 + 访问 token，验证通过后记忆到 Keychain
  - 广域网：`/viewer/gate` → `/viewer/login` 换 `bt`，再按 `bt` 绑定的设备打开查看会话；后续 `/api/v1/*` 由 hawk-server 隧道代理
  - 启动自动连接上次使用的连接；失败回连接列表并展示服务端 message
  - 扫码导入配对载荷（`v/kind/url/token/service_key/key`）与手动输入；配对载荷版本不符时明确提示升级
  - 桌面端设置面板生成配对二维码：局域网（token，支持多网卡选择）/ 广域网（现签只读 hawk-app key，标注服务端同步状态）
  - 连接增删改、重新连接、切换连接、断开
- 浏览
  - 在线首屏不建索引：先出导航/网格占位与 `item/list` 首页，`item/skeleton`、分类/标签与计数后台补齐；完整骨架到达后替换首屏占位
  - 齐行网格（算法与 web `layout.ts` 一致）+ `LazyVStack` 懒渲染；骨架 + 分页详情分离
  - 全屏预览：双指缩放、双击缩放、放大后拖动、左右滑动切换、下拉关闭
  - 保存原图到系统相册（写 `PHPhotoLibrary` 添加权限）
  - 搜索（名称/备注）、排序（修改时间/名称/大小/评分 × 升降序）
  - 筛选：评分、短边像素区间、调色板颜色
  - 侧栏：全部/根目录/未分类/未标签/回收站、文件夹树、分类、标签与计数
  - iPad `NavigationSplitView`；iPhone 侧栏为半屏抽屉
  - 下拉刷新 + 回前台节流刷新
- 主题：沿用 web 的深色变量（`#1e1e1e` / `#252526` / `#2d2d30` / `#4f8cff`），卡片星标、元信息布局对齐桌面端

## 未实现 / 后续里程碑

- 本地索引（GRDB / SQLite）与离线浏览、SSE 增量同步（M2）
- 首屏快照缓存（二次启动/切换库秒开；轻量缓存，不是完整索引）
- `bt` 过期后的自动重换（当前 401 展示错误，重新连接重新登录）
- App Intents / Spotlight 语义索引（M2）
- Foundation Models 自然语言查询、Vision 图像理解（M3）
- 写回（标签/备注）与设备内 iroh 直连（M4）
- 锁视图解锁 UI（当前锁定的文件夹/分类/标签会展示服务端 403 message）
- 单元测试 target（Xcode：`File > New > Target… > Unit Testing Bundle`，target 名 `hawk-iosTests`）

## 开发与自检

```bash
# 格式化（提交前本地跑；配置在 .swift-format）
xcrun swift-format format --in-place --recursive hawk-ios
xcrun swift-format lint --strict --recursive hawk-ios

# 构建（不需要设备与签名）
xcodebuild -scheme hawk-ios -destination 'generic/platform=iOS Simulator' build

# 单测（测试 target 建好后可用）
xcodebuild test -scheme hawk-ios -destination 'platform=iOS Simulator,name=iPhone 17'
```

本地冒烟（Debug 构建）可用环境变量预置一条连接，跳过手动输入：

```bash
SIMCTL_CHILD_HAWK_IOS_DEBUG_PROFILE='{"id":"11111111-1111-1111-1111-111111111111","kind":"lan","name":"测试素材库","url":"http://127.0.0.1:27372","token":"<viewer-token>","serviceKey":"","appKey":""}' \
  xcrun simctl launch booted com.soryu-ryouji.hawk
```

`#if DEBUG` 限定，Release 构建不包含该路径。追加 `SIMCTL_CHILD_HAWK_IOS_DEBUG_PREVIEW=1` 可在列表加载后自动打开首图预览，用于检查预览/缩放布局。真机冒烟仍需覆盖：

1. 局域网：桌面端开 `[web]`，App 手填或扫码导入地址 + token，浏览缩略图与预览
2. 广域网：填服务端地址 + service-key + hawk-app key，进入绑定的在线设备
3. 相册保存：首图保存到相册，确认是原图而非 1024 缩略图
4. 回前台：切后台再回来，确认列表刷新且不白屏

`ATS` 保持默认（不加 `NSAllowsArbitraryLoads`）。局域网请填 IP 字面量（如 `192.168.1.5:27372`）；若将来用 `.local` 主机名或 mDNS 发现，需要另行补 `NSAppTransportSecurity` / `NSBonjourServices`。

## 源码结构

```text
hawk-ios/hawk-ios/
├── HawkApp.swift            App 入口
├── App/
│   ├── AppModel.swift       启动状态机、连接编排
│   └── AppTheme.swift       深色主题与颜色工具
├── Networking/
│   ├── APIClient.swift      信封解包、Bearer、缩略图/原图 URL、错误映射
│   └── Models.swift         REST DTO、查询状态、视图枚举
├── Connection/
│   ├── ConnectionProfile.swift
│   ├── ConnectionStore.swift
│   ├── ConnectionLauncher.swift  局域网 / 广域网 gate-login-session
│   ├── KeychainStore.swift
│   └── PairingPayload.swift
├── Library/
│   ├── LibraryStore.swift   首屏分页/骨架/详情/查询/计数
│   └── JustifiedLayout.swift 齐行布局纯函数
├── Support/
│   ├── ImageStore.swift     缩略图/原图降采样与 NSCache
│   ├── PhotoSaver.swift
│   └── Format.swift
└── Views/                   连接、侧栏、网格、筛选、预览、设置、扫码
```

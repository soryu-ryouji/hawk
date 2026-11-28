# hawk-ios

iPhone / iPad 原生客户端（SwiftUI）。设计文档见 [../docs/frontend/ios-app.md](../docs/frontend/ios-app.md)。

## 环境

- Xcode 27（iOS 27.1 SDK），部署目标 iOS 26.0
- 真机调试需登录 Apple ID（免费账号签名 7 天有效；TestFlight 分发需付费开发者账号）

## 工程现状

| 项 | 值 |
| -- | -- |
| 工程 / target / scheme | `hawk-ios.xcodeproj` / `hawk-ios` + `hawk-ios-share` / 同名 scheme（已共享，进仓库） |
| 源码目录 | `hawk-ios/hawk-ios/`（主 App）与 `hawk-ios/hawk-ios-share/`（Share Extension），均为 `PBXFileSystemSynchronizedRootGroup`：新增/重命名文件直接改磁盘，不用改 `project.pbxproj` |
| App Group | `group.com.soryu-ryouji.hawk`（主 App 写当前库的地址与 AccessKey，分享扩展只读并直连 daemon HTTP API）；需付费开发者账号 |
| 部署目标 | iOS 26.0 / iPadOS 26.0（Foundation Models 文本能力 26.0，图片理解 27.0，按 `#available` 分支） |
| 设备 | iPhone + iPad 单 target（`TARGETED_DEVICE_FAMILY = "1,2"`） |
| Bundle ID | `com.soryu-ryouji.hawk` |
| Swift | Swift 6 语言模式；`SWIFT_DEFAULT_ACTOR_ISOLATION = MainActor`、`SWIFT_APPROACHABLE_CONCURRENCY = YES` |
| Info.plist | 由构建设置生成（`GENERATE_INFOPLIST_FILE = YES`），键走 `INFOPLIST_KEY_*`，不落文件 |
| 平台 | 仅 `iphoneos iphonesimulator`（不构建 macOS / visionOS） |
| 原生依赖 | 无第三方原生依赖，纯 URLSession HTTP；GRDB 计划在 M2 引入 |

新增 Info.plist 键时改 target 的 Build Settings（`INFOPLIST_KEY_<Key>`），不要改成物理 plist 文件。当前配置了：

- `NSLocalNetworkUsageDescription`：局域网直连素材库
- `NSCameraUsageDescription`：扫描配对二维码
- `NSPhotoLibraryAddUsageDescription`：保存素材原图到相册（仅添加，不读取相册）
- `UIUserInterfaceStyle = Dark`：与桌面/web 深色主题一致

忽略规则在本目录的 `.gitignore`（与 `hawk-daemon/`、`hawk-app/` 的惯例一致，子项目自管）；仓库根 `.gitignore` 只放跨项目条目。

## 已实现

### 连接

- 纯局域网直连：候选地址依次读取 `/api/v1/app/lan/connection-info`，校验 `protocol=3` 后用 AccessKey 直接访问 daemon HTTP API（默认端口 27375）
- V3 配对载荷：`v=3`、可选 `target_library_id`、`access_key` 与 `lan_endpoints`；AccessKey 存 Keychain
- 手动填写局域网地址（默认 27375）+ AccessKey；首个候选失败后自动尝试其余地址
- 扫码、剪贴板导入与手动填写；载荷版本或必填字段不符时明确提示
- 连接阶段可视化（读取连接信息 → 索引就绪）与取消；失败回连接列表并展示服务端错误

### 浏览

- iPhone：底部 Tab（素材 / 浏览）：素材页是网格 + 搜索，浏览页是智能分组/文件夹树/分类/标签的推入式列表
- iPad：`NavigationSplitView`；横屏常驻侧栏，竖屏收起（detail 工具栏的系统按钮可随时拉出）；单栏（Split View / 窄窗口）自动退化为 Tab
- 在线首屏不建索引：库信息 + 隐藏集 + 锁集到了就进主界面，`item/list` 首页与 `item/skeleton` 并发，分类/标签与计数后台补齐
- 齐行网格 + `LazyVStack` 懒渲染；滚动位置按视图记忆（切走再回来仍在原处）
- 全屏预览（相册式手势）：横向翻页跟手（首尾页橡皮筋）、双击/双指缩放、放大后单指平移（越界回弹）、未放大时下拉关闭；卡片到预览的缩放转场；保存到相册、系统分享、素材信息
- 相册分享到素材库（Share Extension）：照片 App → 分享 → hawk → 选分类/标签（均可多选、可不选）→ 按当前连接的地址与 AccessKey 直连 daemon 存入素材库；HEIC 自动转 PNG（无损，JPEG/PNG 原样不重编码），最多 20 张，串行上传带进度；同内容重复跳过，同名不同内容明确报错，缺导入权限/凭证失效/网络不通都有提示
- 搜索（名称/备注）、排序、筛选（评分、短边像素区间、调色板颜色）、下拉刷新
- 排序偏好按视图（文件夹/分类/标签 scope）存服务端 `.hawk/view.toml`，与桌面端同步；viewer/importer 不写回

### 锁与实时性

- 锁集 `lock/list` 进 bootstrap；锁定视图展示解锁界面（密码 → 票据），票据以 `X-Hawk-Unlock` 头与媒体 URL 的 `?unlock=` 附带
- 侧栏对锁定条目显示锁图标，解锁后可长按「重新锁定」；工具栏也可「重新锁定此视图」
- SSE `/api/v1/events` 使用 Bearer：`item.*` / `items.*` 更新详情与媒体版本，成员变化刷新骨架与计数，`locks.changed` / `global_filter.changed` / `library.updated` / `task.progress` / `lan.access.changed` 就地应用；断线按退避重连并对齐权限、锁、隐藏集、分类与计数；前后台切换自动停/启订阅

## 未实现 / 后续里程碑

- 本地索引（GRDB / SQLite）与离线浏览（M2）
- 首屏快照缓存（二次启动/切换库秒开）
- App Intents / Spotlight 语义索引（M2）
- Foundation Models 自然语言查询、Vision 图像理解（M3）
- 写回（标签/备注）（M4）

协议回归测试已就位：`hawk-iosTests` 覆盖 V3 局域网配对、Bearer/URL 凭证边界、查询 DTO、扫描间隔与连接信息解析。

## 开发与自检

```bash
# 在 hawk-ios/ 目录执行
# 格式化（提交前本地跑；配置在 .swift-format）
xcrun swift-format format --in-place --recursive hawk-ios
xcrun swift-format lint --strict --recursive hawk-ios

# 构建（不需要设备与签名）
xcodebuild -scheme hawk-ios -destination 'generic/platform=iOS Simulator' build

# 协议单测
xcodebuild test -scheme hawk-ios -destination 'platform=iOS Simulator,name=iPhone 17'
```

本地冒烟（Debug 构建）可用环境变量预置连接与落点，跳过手动输入：

```bash
SIMCTL_CHILD_HAWK_IOS_DEBUG_PROFILE='{"id":"11111111-1111-1111-1111-111111111111","kind":"lan","name":"测试素材库","url":"http://127.0.0.1:27375","accessKey":"<access-key>"}' \
  xcrun simctl launch booted com.soryu-ryouji.hawk
```

| 变量 | 作用 |
| -- | -- |
| `HAWK_IOS_DEBUG_PROFILE` | 预置并激活一条连接（Keychain 里的旧连接不会随卸载清除，此变量每次都覆盖同 id 记录） |
| `HAWK_IOS_DEBUG_TAB` | `browse` 时直接落在浏览 Tab |
| `HAWK_IOS_DEBUG_FOLDER` | 直接打开指定文件夹（iPhone 推入 / iPad 选中） |
| `HAWK_IOS_DEBUG_PREVIEW` | `1` 自动打开首图预览，`info` 同时打开素材信息 |
| `HAWK_IOS_DEBUG_SHEET` | `filter` / `settings` 自动弹出对应面板 |
| `HAWK_IOS_DEBUG_UNLOCK` | 自动填入密码走一遍解锁链路（模拟器无法注入键盘输入） |

均在 `#if DEBUG` 内，Release 构建不含这些路径。真机冒烟仍需覆盖：

1. 局域网：扫码或手填 `http://<IP>:27375` + AccessKey，浏览缩略图、原图与 SSE；验证错误 AccessKey、daemon 未启动与多候选地址回退
2. 相册保存与分享：主 App 保存的是原图；Share Extension 验证局域网导入、同内容跳过与同名冲突报错
3. 锁定视图：输入密码解锁、错误密码保留服务端提示、退出重进需重新解锁
4. 权限与前台：运行中降级 AccessKey 后界面角色更新；切后台再回来，列表、锁集和隐藏集刷新且不白屏

`ATS` 保持默认（不加 `NSAllowsArbitraryLoads`）。局域网请填 IP 字面量（如 `192.168.1.5:27375`）；若将来用 `.local` 主机名或 mDNS 发现，需要另行补 `NSAppTransportSecurity` / `NSBonjourServices`。

## 源码结构

```text
hawk-ios/hawk-ios/
├── HawkApp.swift            App 入口
├── App/
│   ├── AppModel.swift       启动状态机、连接编排、前后台切换
│   ├── AppTheme.swift       深色主题、卡片/间距常量
│   └── Haptics.swift        触感反馈
├── Networking/
│   ├── APIClient.swift      信封解包、Bearer 鉴权、解锁票据头、SSE 会话、错误映射
│   ├── Models.swift         REST DTO、查询状态、视图/锁/排序偏好模型
│   ├── EventStream.swift    SSE 帧解析
│   └── UnlockTicketStore.swift  解锁票据（按维度:名称）
├── Connection/
│   ├── ConnectionProfile.swift
│   ├── ConnectionStore.swift
│   ├── ConnectionLauncher.swift  局域网候选连接、协议校验与启动等待
│   ├── KeychainStore.swift
│   ├── ShareTarget.swift  分享给扩展的当前库地址与 AccessKey
│   └── PairingPayload.swift
├── Library/
│   ├── LibraryContext.swift 会话级共享状态：库信息/分类树/计数/锁/隐藏集/排序偏好/SSE
│   ├── GridStore.swift      单个网格页的视图、查询、骨架与分页详情
│   └── JustifiedLayout.swift 齐行布局纯函数
├── Support/
│   ├── ImageStore.swift     缩略图/原图降采样与 NSCache、相邻页预取
│   ├── MediaExporter.swift  保存到相册 / 分享原图
│   ├── PhotoSaver.swift
│   └── Format.swift
└── Views/
    ├── RootView.swift       阶段切换
    ├── AdaptiveLibraryView.swift  iPhone Tab / iPad 分栏与横竖屏
    ├── BrowseScreen.swift   智能分组/文件夹树/分类/标签
    ├── GridScreen.swift     网格页：工具栏、筛选、锁定态、预览入口
    ├── ItemGrid.swift       卡片、长按菜单、滚动位置、空态
    ├── PreviewView.swift    全屏预览、玻璃工具条、素材信息
    ├── ZoomableImage.swift  预览缩放层：UIScrollView 双击/双指缩放、越界回弹
    ├── LockScreen.swift     解锁界面
    ├── FilterSheet.swift / SettingsView.swift / ConnectView.swift / ProfileFormView.swift / QRScannerView.swift
    └── Components.swift     共享组件（标记、提示条、筛选 chip、计数、锁图标）

hawk-ios/hawk-ios-share/    Share Extension：局域网直连、分类/标签与 multipart 导入
hawk-ios/hawk-iosTests/     V3 配对与网络契约回归测试
```

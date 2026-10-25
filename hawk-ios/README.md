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
| App Group | `group.com.soryu-ryouji.hawk`（主 App 写「当前局域网素材库的地址 + 访问密钥」，分享扩展只读）；需付费开发者账号 |
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

## 已实现

### 连接

- 局域网直连：地址 + 访问 token，验证通过后记忆到 Keychain；二维码可带多个候选地址，客户端逐个探测
- 广域网：`/viewer/gate` → `/viewer/login` 换 `bt` → `/viewer/session` 打开查看会话；`bt` 过期时 401 自动重换一次并重开会话（无需手动「重新连接」）
- 扫码导入配对载荷（`v/kind/url/token/service_key/key`）、从剪贴板导入、手动填写；载荷版本不符时明确提示升级
- 连接阶段可视化（验证 → 登录 → 会话 → 索引就绪）与取消；失败回连接列表并展示服务端 message

### 浏览

- iPhone：底部 Tab（素材 / 浏览）：素材页是网格 + 搜索，浏览页是智能分组/文件夹树/分类/标签的推入式列表
- iPad：`NavigationSplitView`；横屏常驻侧栏，竖屏收起（detail 工具栏的系统按钮可随时拉出）；单栏（Split View / 窄窗口）自动退化为 Tab
- 在线首屏不建索引：库信息 + 隐藏集 + 锁集到了就进主界面，`item/list` 首页与 `item/skeleton` 并发，分类/标签与计数后台补齐
- 齐行网格 + `LazyVStack` 懒渲染；滚动位置按视图记忆（切走再回来仍在原处）
- 全屏预览（相册式手势）：横向翻页跟手（首尾页橡皮筋）、双击/双指缩放、放大后单指平移（越界回弹）、未放大时下拉关闭；卡片到预览的缩放转场；保存到相册、系统分享、素材信息
- 相册分享到素材库（Share Extension）：照片 App → 分享 → hawk → 选分类/标签（均可多选、可不选）→ 存进局域网素材库；HEIC 自动转 PNG（无损，JPEG/PNG 原样不重编码），最多 20 张，串行上传带进度；缺写权限/凭证失效/网络不通都有明确提示
- 搜索（名称/备注）、排序、筛选（评分、短边像素区间、调色板颜色）、下拉刷新
- 排序偏好按视图（文件夹/分类/标签 scope）存服务端 `.hawk/view.toml`，与桌面端同步；只读 token 不写回

### 锁与实时性

- 锁集 `lock/list` 进 bootstrap；锁定视图展示解锁界面（密码 → 票据），票据以 `X-Hawk-Unlock` 头与媒体 URL 的 `?unlock=` 附带
- 侧栏对锁定条目显示锁图标，解锁后可长按「重新锁定」；工具栏也可「重新锁定此视图」
- SSE `/api/v1/events`：`item.*` / `items.*` 增量更新详情，`locks.changed` / `global_filter.changed` / `library.updated` / `task.progress` 就地应用；断线按退避重连并全量对齐；前后台切换自动停/启订阅

## 未实现 / 后续里程碑

- 本地索引（GRDB / SQLite）与离线浏览（M2）
- 首屏快照缓存（二次启动/切换库秒开）
- App Intents / Spotlight 语义索引（M2）
- Foundation Models 自然语言查询、Vision 图像理解（M3）
- 写回（标签/备注）与设备内 iroh 直连（M4）
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

本地冒烟（Debug 构建）可用环境变量预置连接与落点，跳过手动输入：

```bash
SIMCTL_CHILD_HAWK_IOS_DEBUG_PROFILE='{"id":"11111111-1111-1111-1111-111111111111","kind":"lan","name":"测试素材库","url":"http://127.0.0.1:27372","token":"<viewer-token>","serviceKey":"","appKey":""}' \
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

1. 局域网：桌面端开 `[web]`，App 手填或扫码导入地址 + token，浏览缩略图与预览
2. 广域网：填服务端地址 + service-key + hawk-app key，进入绑定的在线设备
3. 相册保存与分享：保存的是原图而非 1024 缩略图
4. 锁定视图：输入密码解锁、退出重进需重新解锁
5. 回前台：切后台再回来，列表刷新且不白屏

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
│   ├── APIClient.swift      信封解包、凭证重换、解锁票据头、SSE 会话、错误映射
│   ├── Models.swift         REST DTO、查询状态、视图/锁/排序偏好模型
│   ├── EventStream.swift    SSE 帧解析
│   └── UnlockTicketStore.swift  解锁票据（按维度:名称）
├── Connection/
│   ├── ConnectionProfile.swift
│   ├── ConnectionStore.swift
│   ├── ConnectionLauncher.swift  局域网 / 广域网 gate-login-session 与自动重换
│   ├── KeychainStore.swift
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
```

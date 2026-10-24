# hawk iOS 客户端设计

iPhone / iPad 原生客户端的设计与路线图。约束来源：[architecture.md](../architecture.md)（前后端解耦）、[server-rest-api-v1.md](../backend/server-rest-api-v1.md)（素材库接口）、[remote-protocol.md](../backend/remote-protocol.md)（广域网契约）。

## 目标与非目标

**目标**

- 在 iPhone / iPad 上原生浏览自己的素材库：局域网直连设备 daemon，广域网经自托管 hawk-server 查看面
- 接入 Apple 智能生态：素材进 Spotlight 语义索引与 Siri（App Intents）、本地模型做自然语言检索与标签建议（Foundation Models）、图像理解（Vision / 本地多模态）
- 离线可读：本地索引与缩略图缓存，弱网/断网仍能浏览与搜索

**非目标（M4 之前）**

- 写操作（导入、删除、改标签）：只读；写回见「写回与安全边界」
- 素材库管理（库设置、扫库、回收站清空）
- Android：M1 已按同一套契约独立实现（Kotlin + Compose），不提前抽象跨平台共享层，见 [Android 客户端设计](android-app.md)
- 替代桌面端：本客户端是「查看 + 查找」，不是管理入口

## 为什么原生，为什么不做 WebView 壳

需求来源是「后期接入 Apple AI / 安卓厂商 AI 生态」，这直接排除了 WebView 壳：

- App Intents 实体、Spotlight 语义索引、Foundation Models 全部是原生 API，WebView 里拿不到
- 现有 web 查看器在移动浏览器已可用（触屏手势、iPad 布局、保存原图，见 `hawk-app/scripts/test-mobile-web.mjs` 冒烟），包一层 WebView 没有增量价值，且 App Store 4.2 有判定风险
- 原生客户端不链接任何 hawk 的 AGPL 代码（只走 HTTP / QUIC 协议），许可证自行决定、可闭源上架；依赖的 iroh-ffi 为 MIT/Apache（需在 App 内声明）

## 平台范围

| 项 | 决策 | 说明 |
| -- | ---- | ---- |
| 部署目标 | iOS 26.0 / iPadOS 26.0 | 项目无存量用户，不为旧系统留兼容分支；Foundation Models 文本能力 iOS 26 起可用，图片理解 iOS 27 起（部分能力需 `#available` 分支）；iOS 26 起可用新 SwiftUI 与 Liquid Glass 外观 |
| 设备家族 | Universal（iPhone + iPad 单 target，`TARGETED_DEVICE_FAMILY = "1,2"`） | iPad 靠 `NavigationSplitView` 与 size class 适配，不建独立 target |
| 语言 | Swift 6（strict concurrency） | SwiftUI 声明式 UI，无 UIKit 视图控制器 |
| 工程 | `hawk-ios/`，提交 `.xcodeproj` + shared scheme（scheme 名 `hawk-ios`） | 不引入 XcodeGen / Tuist，项目文件直读直改 |
| 源码组织 | 同步文件夹（`PBXFileSystemSynchronizedRootGroup`） | 新增/重命名文件直接改磁盘，不产生 pbxproj 冲突 |
| 格式化 | `xcrun swift-format`（Xcode 自带），配置 `hawk-ios/.swift-format` | 与 `cargo fmt` / `prettier` 同级：写入式格式化 + `lint --strict` 门禁 |
| 依赖 | SwiftPM | M2 引入 GRDB（SQLite）；M4 视需要引入 IrohLib |
| 不做 | Catalyst、KMP、Flutter、WebView 壳 | 与「原生 AI 能力」目标冲突或无收益 |

**AI 可用性门槛**（已按 Xcode 27.1 的 iOS 27.1 SDK 的 `@available` 标注核实，都要做能力检测，不能假设存在）：

| 能力 | 起始系统版本 | 备注 |
| ---- | ------------ | ---- |
| App Intents / Spotlight 语义索引 | 全机型 | 索引由系统侧完成 |
| Foundation Models 文本推理（`SystemLanguageModel` / `LanguageModelSession`） | iOS 26.0 | 另需设备支持 Apple Intelligence |
| Foundation Models 图片附件（`Attachment` / `ImageAttachment`） | **iOS 27.0** | 部署目标为 26，所以图片理解必须走 `#available(iOS 27, *)` 分支 |
| Vision（OCR / 分类 / 分割） | 全机型 | iOS 26 设备上的图像理解降级路径 |

结论：部署目标 26.0，图片理解按 iOS 27 分支实现；不允许为迁就 26 而复制一套图像处理逻辑——iOS 26 上一律走 Vision 单独出标签候选。

## 连接形态

| 形态 | 数据通路 | 鉴权 | 适用 | 代价 |
| -- | -- | -- | -- | -- |
| 局域网直连 | App → `http://<局域网IP>:<peer_port>` → daemon | `[web].peer_keys` 中对应读/写权限的直连密钥（网站关闭也可用） | 同网段，带宽最大、时延最低 | 需手动填地址或扫码；iOS 首次弹「本地网络」权限；扫码收到多个候选地址时自动逐个探测 |
| 广域网经查看面 | App → `https://<server>` `/viewer/gate` → `/viewer/login` → `bt`，其余 `/api/v1/*` 由服务端隧道代理 | service-key + hawk-app key 换 `bt` | 任意网络，零后端改动 | 素材流量过自建服务器（带宽 + 服务端可见明文） |
| 设备内 iroh 直连（M4 起评估） | App 内 iroh endpoint → 打洞/中继 → 目标 daemon | 手机作为池内设备注册（endpoint_id / 心跳 / WSS `session.request`） | 大量浏览、AI 高频小查询 | 需引入 iroh-ffi、实现 B 侧隧道 + HTTP/1.1 over 双向流、会话状态机 |

三点约定：

1. **M1–M3 只做前两条**，它们合起来已覆盖「在家里看」与「在外面看」。iroh 直连是带宽优化，不是功能前提。
2. iOS 访问局域网明文 HTTP：需实测 ATS 行为（IP 字面量通常不受 ATS 约束），但**必须**在 Info.plist 声明 `NSLocalNetworkUsageDescription`；若将来支持 mDNS 发现，需加 `NSBonjourServices`（daemon 目前为桌面 hawk-app 互联提供 UDP 发现，未接入移动端，也不广播 mDNS）。
3. App 进入后台后 QUIC/TCP 连接会被挂起，回前台一律重连；不申请后台模式。

## 鉴权与配对

- 长期凭证存 Keychain（accessibility：`kSecAttrAccessibleAfterFirstUnlock`）：局域网直连 token、service-key、hawk-app key。`bt` 是短生命周期令牌，可只在内存持有（当前实现不落盘，重启重新登录）
- 配对主路径是**桌面端出二维码**，App 扫码导入（桌面端设置面板已提供入口）：
  - 局域网：`{ "v": 1, "kind": "lan", "urls": ["http://192.168.1.5:27375", "http://10.0.0.5:27375"], "token": "..." }`
  - 广域网：`{ "v": 1, "kind": "remote", "url": "https://hawk.example.com", "service_key": "hs_...", "key": "hk_..." }`
  - `v` 为载荷版本，解码失败给「二维码版本不支持，请升级」而不是静默失败
- 手动输入地址 + token 作为兜底（无桌面端可用时）
- `bt` 有效期由服务端决定。设计目标是用 Keychain 里的 service-key + key 自动重换一次；**当前实现**在 401/过期时展示错误，由用户点「重新连接」重新走 `gate`/`login`（见「实现进度」差异）

## 在线首屏加载（不依赖本地索引）

在线浏览时手机不建索引：素材索引在 daemon（桌面端 / hawk-server 宿主机）上，手机只做查询与渲染。M1 的首屏策略：

1. `app/info` + `app/startup` 就绪、远端会话建立后，只等 `library/info` 与 `global_filter/list`，立即进主界面——导航、侧栏和网格占位先出现。
2. 后台并发：
   - `item/list` 第一页（48 条）先出真实卡片；
   - `item/skeleton` 建立完整虚拟网格（滚动条总高与跳转）；
   - `folder/list` / `category/list` / `tag/list` 填侧栏。
3. 完整骨架到达后替换首屏分页骨架；首屏已取到的详情复用，第一页不重复请求。
4. 5 个智能条目计数（全部/根目录/未分类/未标签/回收站）最后加载，侧栏先隐藏计数，不阻塞首屏。

因此「手机上先建索引」不是在线浏览的前提。M2 的 GRDB 索引用于离线、Spotlight 与 AI；若要让二次启动/切换库也秒开，需要的是首屏快照缓存（裁剪后的骨架 + 分类 + 计数），不是完整索引。首次加载仍取决于 daemon 出首屏分页与骨架的速度——首屏分页已先展示，完整骨架在后台补齐。

## 界面结构

按 iOS 习惯而非桌面布局搬迁：一级入口用系统导航组件，控件尺寸与字号跟随系统设置。

| 位置 | iPhone | iPad |
| -- | -- | -- |
| 一级导航 | 底部 `TabView`：素材（网格 + 搜索）/ 浏览（智能分组、文件夹树、分类、标签） | `NavigationSplitView`：左栏为同一套浏览列表，右栏为网格 |
| 横竖屏 | 网格按宽度齐行，Tab 条自动收成紧凑形态 | 横屏常驻侧栏并加宽网格；竖屏收起侧栏（detail 工具栏的系统按钮拉出）；Split View 单栏退化为 Tab |
| 视图切换 | 浏览页点条目推入网格，返回手势回列表 | 左栏选中即换右栏内容，滚动位置按视图记忆 |

交互约定：

- 网格卡片点击进预览，长按出系统菜单（打开、保存到相册、分享原图、素材信息、复制名称）
- 预览为横向分页 + 双击/双指缩放 + 下拉关闭，卡片到全屏用缩放转场；底部玻璃工具条放保存/分享/信息
- 搜索栏常驻网格页顶部（iOS 26 的搜索抽屉），排序与筛选合并进一个菜单，避免工具栏拥挤
- 字号全部用语义字号（`.body` / `.subheadline` / `.caption`），网格元信息高度随动态字体缩放；保存、解锁等状态变化才触发触感反馈
- 锁定视图就地展示密码输入，不弹模态；解锁后原地重查，不丢失当前导航位置

## 本地索引（M2 核心）

AI 能力与离线浏览都依赖 App 侧有一份素材元数据，这一层必须自建。

- 存储：GRDB（SQLite），库文件放 `Application Support`，可整体删除后重建（与 daemon 的 `index.db` 同语义：派生缓存，不是权威数据）
- 表：`item(id, name, ext, width, height, size, star, tags, categories, folders, paths, annotation, modification_time, library_id)` + `sync_state(library_id, last_sync_at, server_generation)`
- 注水：启动先用本地库渲染（秒开），再增量对账
  - 骨架：`POST /api/v1/item/skeleton`（含 id/path/宽高/star/size，全量、不分页）
  - 元数据：`POST /api/v1/item/list` 分页补齐——skeleton **不含** tags / annotation / modification_time，万级素材按 50/页需要 200 次请求
  - 增量：订阅 SSE `/api/v1/events` 的 `item.*` 事件更新单条；断线重连后按 `modification_time` 对账
- 若全量拉取被证明是瓶颈（万级以上 + 高延迟链路），再考虑给 daemon 加批量元数据端点；在此之前不做

## AI 分层

分三层落地，先后顺序就是收益/成本比：

| 层 | 能力 | API | 门槛 | 阶段 |
| -- | ---- | --- | ---- | ---- |
| L1 系统发现与操作 | 素材可被 Spotlight 搜到、Siri 可「在 hawk 里找…」 | App Intents（entity schema 直接进 Spotlight 语义索引），按 Apple 的 photo app 集成指南选系统预置 schema | 全机型 | M2 |
| L2 本地推理 | 自然语言 → 结构化查询（iOS 26）；缩略图 → 标签建议（iOS 27 起直接看图，iOS 26 走 L3） | Foundation Models（guided generation + tool calling）；图片用 `Attachment` 附件，失败降级关键词 | iOS 26+（图片理解 iOS 27+）且支持 Apple Intelligence | M3 |
| L3 图像理解 | OCR 文本、场景分类、主体分割 | Vision（`VNRecognizeTextRequest`、分类请求、分割请求） | 全机型 | M3 并行 |

设计约束：

- **L2 只产出 `item/list` 的请求参数结构**（keywords / tags / categories / folders / star / 时间区间 / ext / min_side），不产出 URL、不直接调接口——参数结构可测、可回放、可离线评估
- 图片理解用缩略图（1024 webp，daemon 已产出），不拉原图；原图仅预览时按 ImageIO 降采样解码
- 冷启动时延：Spotlight / Siri 唤起可能冷启动 App，网络侧不可同步等待——先返回本地索引结果，远端对账完成后再刷新；单次意图响应预算 1s 内出首屏
- 标签建议只进本地「建议池」，不自动写库（见下节）

## 写回与安全边界

现状（不改动即成立）：

- 局域网 write token：可走 `item/update` 等全部写端点
- 广域网：`hawk-daemon/src/api/mod.rs` 的 `is_remote_writer_allowed` 仅放行 `item/add`（禁 `path`）、`item/upload` 与 `folder/create`，其余写端点 403 `READ_ONLY`

决策：

1. M3 的 AI 标签建议**只展示、不写回**，用户确认后由本地权限判定走哪条路
2. 广域网写回需要扩白名单（仅 `item/update` 的 `tags` / `annotation` 字段），属于安全边界变更：要单独设计、单独测试、同步更新 [remote-access.md](../backend/remote-access.md) 的权限分层表与 [远程访问协议](../backend/remote-protocol.md)
3. 在没有任何真实使用数据前不动白名单

## 实现进度

截至当前仓库版本，M0 与 M1 的只读主链路已落地，代码在 `hawk-ios/hawk-ios/`：

| 能力 | 状态 | 说明 |
| ---- | ---- | ---- |
| 局域网直连 + token 验证 | ✅ | Keychain 记忆、启动自动连接、失败回连接列表 |
| 广域网查看面 | ✅ | `gate` → `login` 换 `bt` → `viewer/session`；`bt` 绑定单设备，自动打开，不再做设备选择 |
| 扫码 / 手动配对 | ✅ | iOS 扫码与手填；桌面端局域网/广域网设置面板可生成配对二维码（局域网含 token，广域网现签只读 key 并标注同步状态） |
| 在线首屏（不依赖本地索引） | ✅ | 库信息+隐藏集后立即进主界面；`item/list` 首页与 `item/skeleton` 并发，首屏分页先出，分类/标签与计数后台补齐 |
| 齐行网格 + 分页详情 | ✅ | 骨架 + `item/list` 窗口分页 + `LazyVStack`；首屏分页与骨架共用详情缓存，缩略图尺寸可调 |
| 全屏预览 | ✅ | 缩放/拖动/左右切换/下拉关闭；原图降采样解码 |
| 保存到相册 | ✅ | 保存原图数据（`PHPhotoLibrary` 添加权限） |
| 搜索 / 排序 / 筛选 | ✅ | 名称备注搜索、八种排序、评分、短边区间、颜色；排序偏好按视图 scope 存服务端，与桌面端同步 |
| 导航 | ✅ | iPhone 底部 Tab（素材 / 浏览）；iPad `NavigationSplitView`，横屏常驻侧栏、竖屏收起；单栏（Split View）退化为 Tab |
| 下拉刷新 / 回前台刷新 | ✅ | 另订阅 SSE 增量更新，前后台切换自动停/启 |
| 锁视图解锁 | ✅ | `lock/list` + `lock/unlock`，票据走 `X-Hawk-Unlock` 与媒体 `?unlock=`；可重新锁定 |
| 实时事件（SSE） | ✅ | `item.*` / `items.*` / `locks.changed` / `global_filter.changed` / `library.updated` / `task.progress`；断线退避重连后全量对齐 |
| `bt` 过期自动重换 | ✅ | 401 时自动 gate→login→session 重试一次 |
| 本地索引 / 离线 | ⬜ | M2 |
| App Intents / Spotlight | ⬜ | M2 |
| Foundation Models / Vision | ⬜ | M3 |

实现中与设计稿的差异：

- **设备选择**：hawk-app key 在服务端只映射一台设备，`login` 返回的 `device_id` 即目标，客户端直接 `POST /viewer/session`。服务端的 `/viewer/devices` 仍服务于网页版选择器。
- **会话重建**：每次连接都调用 `POST /viewer/session`（服务端对存活会话复用、对死会话重新撮合），不依赖 `GET /viewer/session`，避免旧隧道失效后卡死。
- **实时性**：已订阅 SSE（`/api/v1/events`，token 走查询参数、票据走 `?unlock=`）。单条/批量更新只刷新详情缓存不动骨架，成员变化（增删/移动/锁变更/隐藏集变更）才重查骨架；消费落后被服务端断开后按 1s→15s 退避重连并全量对齐。SSE 归在 M2 的增量同步里，提前落地。
- **bt 不落盘**：服务端换发的浏览器令牌只保存在内存，App 重启后重新走 `gate`/`login`；局域网直连 token、service-key 与 hawk-app key 存 Keychain。
- **bt 过期/401**：请求层捕获 401 后自动重换一次 `bt` 并重开查看会话，失败才回到错误态提示重新连接。
- **二次启动/切换库秒开**：尚未做磁盘首屏快照缓存；当前在线首屏已先出（分页 + 占位），但每次启动/切库仍重新认证并重取首屏。
- **导航**：未做桌面端的前进/后退栈；iPhone 由导航栈与「浏览」Tab 承担，iPad 由侧栏选中项承担。
- **锁定视图**：进入未解锁的锁定视图时不发列表请求，直接展示解锁界面；服务端 403 `LOCKED` 也归到同一界面。票据只存内存，重启需重新解锁。

## 路线图

| 阶段 | 交付物 | 验收标准 | 状态 |
| ---- | ------ | -------- | ---- |
| M0 工程就位 | `hawk-ios/` Xcode 工程、SwiftUI 骨架、Keychain 存储、网络层（URLSession + 信封解包） | 真机可运行；局域网连上 daemon 拿到 `app/info` | ✅ |
| M1 只读浏览 | 连接管理（局域网 / 广域网查看面）、扫码配对、齐行网格 + 虚拟滚动、首屏分页先出、全屏预览（缩放/滑动/下拉关闭）、保存到相册、搜索与筛选、文件夹树 | 1000+ 素材的库滚动不掉帧；广域网从扫码到出图 ≤ 3s（首屏用缩略图） | 🟡 核心与桌面二维码完成，1000+ 性能待压测 |
| M2 本地索引 + 系统集成 | GRDB 索引与离线浏览、App Intents 实体 + Spotlight 语义索引（SSE 增量更新已提前落地） | 飞行模式下可浏览与搜索；Spotlight 能搜到库内素材并跳转 | 🟡 SSE 已接，本地索引未开始 |
| M3 端上 AI | Foundation Models 自然语言 → 查询参数；图片理解出标签候选（iOS 27+ 走 `Attachment`，iOS 26 走 Vision OCR/分类）；能力检测与降级 | 无 Apple Intelligence 的机型功能完整可用；自然语言查询命中率在自建样例集上可评估 | ⬜ |
| M4 写回与直连评估 | 写回确认流（按决策 2）；视实际流量决定是否引入 iroh-ffi 直连 | — | ⬜ |

## 对仓库其他部分的改动

| 位置 | 改动 | 阶段 |
| ---- | ---- | ---- |
| `hawk-ios/` | 新增原生客户端（本文件的设计对象） | M0 |
| `hawk-app`（桌面端） | 生成配对二维码（局域网直连 token / 广域网 service-key + hawk-app key），设置面板加入口 | M1 已完成 |
| `README.md` / `docs/tech-stack.md` / `docs/architecture.md` | 移动端条目与仓库结构 | 已随本文件更新 |
| `hawk-daemon` | 已提供桌面 hawk-app 的 UDP 局域网发现；移动端 mDNS/Bonjour 自动发现与批量元数据端点（仅当全量分页成为瓶颈） | M2 / 视情况 |
| `.github/workflows/ci.yml` | 暂不纳入 iOS 构建（需要 macOS runner + 签名）；本地提交前跑 `xcodebuild build` | — |

## 风险

| 风险 | 影响 | 应对 |
| ---- | ---- | ---- |
| Foundation Models 的分机型能力差异 | iOS 26 只有文本，图片理解要 iOS 27；部分设备不支持 Apple Intelligence | 已核实 SDK 版本门槛（见「AI 可用性门槛」）；能力检测 + Vision 降级路径，UI 不出现“功能缺失”态 |
| iroh-ffi 与 daemon 的 iroh 1.4 版本对齐 | M4 直连可能握手/ALPN 不兼容 | iroh-ffi 镜像 iroh 1.0 稳定面，属 Tier 2 仓库；先用 hello-iroh-ffi 连本机 daemon 验证再排期 |
| iOS 网络限制（本地网络权限、ATS、后台挂起） | 局域网首连失败、回前台断流 | 显式权限说明文案、回前台重连 + 状态恢复 |
| 广域网浏览吃服务器带宽 | 自托管服务器流量成本 | 缩略图优先、预览才取原图、本地缓存；必要时提前 M4 |
| App Store 审核（4.2 / 内容政策） | 上架风险 | 原生功能充足（离线库、相册、搜索、AI），不是网站包装 |

## 待定项

1. **AI 定位**：只做「查找增强」，还是要做「自动打标写回」——后者决定是否改 `remote_writer` 白名单
2. **局域网自动发现**：是否值得给 daemon 加 mDNS（否则每次都靠扫码）
3. **Android 共享层**（已解决）：Android M1 已独立实现，未引入 KMP；两端重复逻辑稳定后再评估

## 相关文档

- [架构设计](../architecture.md)
- [技术栈](../tech-stack.md)
- [hawk-app 设计](hawk-app.md)（桌面端与 web 查看器，触屏交互语义可参考）
- [REST API V1](../backend/server-rest-api-v1.md)
- [远程访问设计](../backend/remote-access.md) / [远程访问协议](../backend/remote-protocol.md)

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
- Android：等 iOS 稳定后另评估，不提前抽象跨平台共享层
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
| 局域网直连 | App → `http://<局域网IP>:<web.port>` → daemon | `[web]` 的 viewer / write token | 同网段，带宽最大、时延最低 | 需手动填地址或扫码；iOS 首次弹「本地网络」权限 |
| 广域网经查看面 | App → `https://<server>` `/viewer/gate` → `/viewer/login` → `bt`，其余 `/api/v1/*` 由服务端隧道代理 | service-key + hawk-app key 换 `bt` | 任意网络，零后端改动 | 素材流量过自建服务器（带宽 + 服务端可见明文） |
| 设备内 iroh 直连（M4 起评估） | App 内 iroh endpoint → 打洞/中继 → 目标 daemon | 手机作为池内设备注册（endpoint_id / 心跳 / WSS `session.request`） | 大量浏览、AI 高频小查询 | 需引入 iroh-ffi、实现 B 侧隧道 + HTTP/1.1 over 双向流、会话状态机 |

三点约定：

1. **M1–M3 只做前两条**，它们合起来已覆盖「在家里看」与「在外面看」。iroh 直连是带宽优化，不是功能前提。
2. iOS 访问局域网明文 HTTP：需实测 ATS 行为（IP 字面量通常不受 ATS 约束），但**必须**在 Info.plist 声明 `NSLocalNetworkUsageDescription`；若将来支持 mDNS 发现，需加 `NSBonjourServices`（daemon 目前不广播 mDNS，需另行添加）。
3. App 进入后台后 QUIC/TCP 连接会被挂起，回前台一律重连；不申请后台模式。

## 鉴权与配对

- 全部凭证存 Keychain（accessibility：`kSecAttrAccessibleAfterFirstUnlock`）：局域网 token、`bt`、service-key、hawk-app key
- 配对主路径是**桌面端出二维码**，App 扫码导入（新增桌面端能力）：
  - 局域网：`{ "v": 1, "kind": "lan", "url": "http://192.168.1.5:27372", "token": "..." }`
  - 广域网：`{ "v": 1, "kind": "remote", "url": "https://hawk.example.com", "service_key": "hs_...", "key": "hk_..." }`
  - `v` 为载荷版本，解码失败给「二维码版本不支持，请升级」而不是静默失败
- 手动输入地址 + token 作为兜底（无桌面端可用时）
- `bt` 有效期由服务端决定，过期或 401 时用 Keychain 里的 service-key + key 自动重换一次；再失败才提示重新配对

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
- 广域网：`hawk-daemon/src/api/mod.rs` 的 `is_remote_writer_allowed` 仅放行 `item/add`（禁 `path`）与 `folder/create`，其余写端点 403 `READ_ONLY`

决策：

1. M3 的 AI 标签建议**只展示、不写回**，用户确认后由本地权限判定走哪条路
2. 广域网写回需要扩白名单（仅 `item/update` 的 `tags` / `annotation` 字段），属于安全边界变更：要单独设计、单独测试、同步更新 [remote-access.md](../backend/remote-access.md) 的权限分层表与 [远程访问协议](../backend/remote-protocol.md)
3. 在没有任何真实使用数据前不动白名单

## 路线图

| 阶段 | 交付物 | 验收标准 |
| ---- | ------ | -------- |
| M0 工程就位 | `hawk-ios/` Xcode 工程、SwiftUI 骨架、Keychain 存储、网络层（URLSession + 信封解包） | 真机可运行；局域网连上 daemon 拿到 `app/info` |
| M1 只读浏览 | 连接管理（局域网 / 广域网查看面）、设备选择、齐行网格 + 虚拟滚动、全屏预览（缩放/滑动/下拉关闭）、保存到相册、搜索与筛选、文件夹树 | 1000+ 素材的库滚动不掉帧；广域网从扫码到出图 ≤ 3s（首屏用缩略图） |
| M2 本地索引 + 系统集成 | GRDB 索引与增量同步、离线浏览、App Intents 实体 + Spotlight 语义索引、二维码配对 | 飞行模式下可浏览与搜索；Spotlight 能搜到库内素材并跳转 |
| M3 端上 AI | Foundation Models 自然语言 → 查询参数；图片理解出标签候选（iOS 27+ 走 `Attachment`，iOS 26 走 Vision OCR/分类）；能力检测与降级 | 无 Apple Intelligence 的机型功能完整可用；自然语言查询命中率在自建样例集上可评估 |
| M4 写回与直连评估 | 写回确认流（按决策 2）；视实际流量决定是否引入 iroh-ffi 直连 | — |

## 对仓库其他部分的改动

| 位置 | 改动 | 阶段 |
| ---- | ---- | ---- |
| `hawk-ios/` | 新增原生客户端（本文件的设计对象） | M0 |
| `hawk-app`（桌面端） | 生成配对二维码（局域网 token / 广域网 service-key + hawk-app key），设置面板加入口 | M1 |
| `README.md` / `docs/tech-stack.md` / `docs/architecture.md` | 移动端条目与仓库结构 | 已随本文件更新 |
| `hawk-daemon` | 可选：mDNS 广播（自动发现局域网库）；批量元数据端点（仅当全量分页成为瓶颈） | M2 / 视情况 |
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
3. **Android 时间点**：半年内跟进则考虑 Rust 共享层（iroh 隧道 + HTTP 客户端 + 缓存），否则 iOS 用 Swift 直写

## 相关文档

- [架构设计](../architecture.md)
- [技术栈](../tech-stack.md)
- [hawk-app 设计](hawk-app.md)（桌面端与 web 查看器，触屏交互语义可参考）
- [REST API V1](../backend/server-rest-api-v1.md)
- [远程访问设计](../backend/remote-access.md) / [远程访问协议](../backend/remote-protocol.md)

# hawk Android 客户端设计

Android 手机 / 平板原生客户端。约束来源：[architecture.md](../architecture.md)（前后端解耦）、[server-rest-api-v1.md](../backend/server-rest-api-v1.md)（素材库接口）、[remote-protocol.md](../backend/remote-protocol.md)（广域网契约）、[iOS 客户端设计](ios-app.md)（同一套连接与首屏策略）。

## 目标与非目标

**目标**

- 在 Android 手机 / 平板上原生浏览素材库：局域网直连 daemon，广域网经自托管 hawk-server 查看面
- 与 iOS M1 相同的只读主链路：连接管理、扫码配对、在线首屏、齐行网格、全屏预览、搜索/筛选、侧栏
- 复用同一套 HTTP 契约，不链接 hawk 的 AGPL 代码

**非目标（M4 之前）**

- 写操作（导入、删除、改标签）、素材库管理
- 本地索引 / 离线浏览 / 端上 AI（M2/M3 评估）
- 与 iOS 共享代码的跨平台层（KMP / Flutter）：当前两端都是薄客户端，共享层只会增加构建与调试成本；等两端功能稳定、重复逻辑真的出现再评估

## 为什么原生，为什么不做 WebView 壳

- 移动浏览器已能访问 web 查看器，WebView 壳没有增量价值
- 后续的系统集成（App Actions / 系统搜索索引、端上模型）只有原生 API 可用
- 原生客户端只走协议，不引入 AGPL 代码，许可证独立

## 平台范围

| 项 | 决策 | 说明 |
| -- | ---- | ---- |
| 部署目标 | minSdk 26（Android 8.0）/ targetSdk 37 | 覆盖绝大多数在网设备；API 26 起可直接用 MediaStore 写入相册 |
| 形态 | 单模块 `app`，单 Activity + Compose | 手机与平板同一套代码；宽屏靠断点切换侧栏形态 |
| 语言 | Kotlin 2.4 + Jetpack Compose（Material 3） | 不使用 XML 布局、Fragment、Navigation Compose（单 Activity 内的状态机已足够） |
| 工程 | `hawk-android/`，Gradle Kotlin DSL | 版本集中在 `gradle/libs.versions.toml` |
| 网络 | OkHttp 5 + kotlinx.serialization | 信封解包、Bearer、`snake_case` 字段显式映射 |
| 图片 | Coil 3（OkHttp fetcher） | 缩略图按视图尺寸解码；原图预览限 4096px；内存 + 512MB 磁盘缓存 |
| 二维码 | CameraX + ZXing core | 不依赖 Google Play Services，模拟器/无 GMS 设备可用 |
| 凭证 | 应用私有 SharedPreferences | 见「鉴权与配对」的取舍说明 |
| 不做 | WebView 壳、KMP/Flutter、Jetpack Navigation | 与目标冲突或无收益 |

## 连接形态

与 iOS 完全一致：

| 形态 | 数据通路 | 鉴权 |
| -- | -- | -- |
| 局域网直连 | App → `http://<局域网IP>:<peer_port>` → daemon | `[web].peer_keys` 中对应读/写权限的直连密钥（网站关闭也可用） |
| 广域网经查看面 | App → `https://<server>` `/viewer/gate` → `/viewer/login` → `bt`，其余 `/api/v1/*` 由服务端隧道代理 | service-key + hawk-app key 换 `bt` |

约定：

1. 局域网明文 HTTP 通过 `android:usesCleartextTraffic="true"` 放行（Android 9+ 默认禁明文）；广域网必须 `https`
2. App 退到后台后连接被挂起，回前台由 `LifecycleResumeEffect` 触发节流刷新；不申请后台服务
3. 模拟器联调用 `adb reverse tcp:<port> tcp:<port>`，App 填 `http://127.0.0.1:<port>`

## 鉴权与配对

- 配对载荷与 iOS 相同：
  - 局域网 `{ "v": 1, "kind": "lan", "url": "...", "urls": ["..."], "token": "..." }`
  - 广域网 `{ "v": 1, "kind": "remote", "url": "...", "service_key": "...", "key": "..." }`
- `v` 不等于 1 时提示升级，不静默失败
- 扫码（CameraX + ZXing）为主路径，手动输入地址 + token 兜底；局域网支持直接粘贴带 `?token=` 的查看器地址
- 凭证存应用私有 SharedPreferences（`ConnectionStore`），`allowBackup=false` 且 `dataExtractionRules` / `fullBackupContent` 排除全部应用数据，凭证不离开设备
  - 与 iOS Keychain 的差异：未做 Keystore 加密。Android 应用沙箱已隔离其他应用，备份通道也已关闭；Root 设备上的读取风险与桌面端 token 文件同级，暂不引入 EncryptedSharedPreferences（库已废弃、增加 Tink 依赖）
- `bt` 只在内存持有，进程重启后重新走 `gate`/`login`；401/过期时展示错误，由用户「重新连接」

## 在线首屏（不依赖本地索引）

与 iOS 相同的三步：

1. `app/info` + `app/startup` 就绪后，只等 `library/info` 与 `global_filter/list`，立即进主界面
2. 后台并发：`item/list` 第一页（48 条）先出真实卡片；`item/skeleton` 建完整虚拟网格；`folder/list` / `category/list` / `tag/list` 填侧栏
3. 完整骨架到达后替换首屏分页；首屏已取到的详情复用；5 个智能条目计数最后加载

## 界面结构

- **连接页**：已保存连接列表（上次使用标记、编辑、删除）、局域网/广域网入口、扫码入口、错误横幅；连接中页可取消
- **主界面**：
  - 手机：`ModalNavigationDrawer`（抽屉 300dp）+ 顶栏（菜单 / 标题 / 筛选 / 排序 / 更多）+ 常驻搜索框 + 激活筛选条
  - 宽屏（≥840dp）：常驻 280dp 侧栏 + 内容区（平板横屏即触发）
  - 侧栏：智能条目与计数、文件夹树（可展开）、分类、标签
- **网格**：`LazyColumn` 按行懒渲染，行内 `Row` 固定宽高（dp），算法与 web `layout.ts` 一致；星标叠在缩略图右上
- **预览**：`HorizontalPager` 左右切换；`transformable(canPan = scale > 1)` 处理双指缩放与放大后拖动，`detectTapGestures` 处理双击缩放，未放大时垂直下拉关闭；顶部渐变浮层放关闭/保存/信息
- **筛选**：底部弹层，评分、短边区间（含 480p–4320p 档位）、调色板颜色；一次性应用，避免每个维度各触发一轮骨架请求
- **连接信息**：底部弹层，展示库/连接/访问级别/服务地址/条目数/当前视图、重连/切换/断开、缩略图尺寸（120–280dp，默认 160dp）、版本信息
- 返回键：预览由 `BackHandler` 关闭；抽屉/弹层由 Material 组件处理；主界面返回退出

## Android 特有实现

- 保存到相册：API 29+ 用 `MediaStore` 的 `RELATIVE_PATH=Pictures/hawk` + `IS_PENDING`，无需权限；API 26–28 申请 `WRITE_EXTERNAL_STORAGE`
- 缩略图重试：新入库素材缩略图可能 404，首次失败后 1.5s 重试最多 3 次（与 iOS 一致）
- 调试预置连接：Debug 构建读取 `/sdcard/Android/data/com.ryouji.hawk/files/hawk_profile.json`（`BuildConfig.DEBUG` 限定，Release 不含），用于模拟器/自动化冒烟

## 实现进度

| 能力 | 状态 | 说明 |
| ---- | ---- | ---- |
| 工程与主题 | ✅ | 深色主题、启动/连接中/主界面状态机 |
| 局域网直连 + token 验证 | ✅ | 应用私有存储记忆、启动自动连接、失败回连接列表 |
| 广域网查看面 | ✅ | `gate` → `login` 换 `bt` → `viewer/session`；`bt` 绑定单设备 |
| 扫码 / 手动配对 | ✅ | CameraX + ZXing；配对载荷版本校验 |
| 在线首屏 | ✅ | 库信息+隐藏集后立即进主界面；首页与骨架并发 |
| 齐行网格 + 分页详情 | ✅ | 骨架 + `item/list` 窗口分页 + `LazyColumn`；缩略图尺寸可调 |
| 全屏预览 | ✅ | 双指/双击缩放、拖动、左右切换、下拉关闭 |
| 保存到相册 | ✅ | MediaStore；26–28 权限处理 |
| 搜索 / 排序 / 筛选 | ✅ | 名称备注搜索、八种排序、评分/短边/颜色 |
| 侧栏导航 | ✅ | 智能条目、文件夹树、分类、标签与计数；手机抽屉 / 宽屏常驻 |
| 下拉刷新 / 回前台刷新 | ✅ | 无 SSE，节流拉取 |
| 本地索引 / 离线 | ⬜ | M2 |
| 系统集成（App Actions / 搜索索引） | ⬜ | M2 评估 |
| 端上 AI | ⬜ | M3 评估 |
| 锁视图解锁 | ⬜ | 当前展示服务端 403 message |

## 与 iOS 的行为差异

- 默认缩略图目标高度 160dp（iOS 为 200pt）：手机宽度下 200 会让多数图片单列，160 更接近桌面端的多列观感；范围同为 120–280
- 没有 iOS 的 `NavigationStack` 返回栈，文件夹层级靠侧栏树与「更多 → 返回上级」；顶栏不显示返回按钮
- 凭证存储为应用私有 SharedPreferences（iOS 为 Keychain），差异与取舍见上
- 预览缩放没有过渡动画（双击/回弹为即时切换），缩放范围 1×–5× 与 iOS 一致

## 路线图

| 阶段 | 交付物 | 验收标准 | 状态 |
| ---- | ------ | -------- | ---- |
| M0 工程就位 | Gradle 工程、Compose 骨架、网络层、连接存储 | 模拟器可运行；局域网连上 daemon 拿到 `app/info` | ✅ |
| M1 只读浏览 | 连接管理、扫码、齐行网格、预览/保存、搜索筛选、侧栏 | 1000+ 素材滚动不卡顿；广域网扫码到出图 ≤ 3s | 🟡 核心完成，1000+ 性能待压测 |
| M2 本地索引 + 系统集成 | Room/SQLite 索引与增量同步、离线浏览、系统搜索索引 | 飞行模式可浏览与搜索 | ⬜ |
| M3 端上 AI | 自然语言检索 / 标签候选（按设备能力降级） | 无 AI 能力的机型功能完整 | ⬜ |
| M4 写回与直连评估 | 写回确认流（受 `remote_writer` 白名单约束）；iroh-ffi 直连评估 | — | ⬜ |

## 对仓库其他部分的改动

| 位置 | 改动 | 阶段 |
| ---- | ---- | ---- |
| `hawk-android/` | 新增 Android 客户端（本文件的设计对象） | M0/M1 |
| `README.md` / `docs/tech-stack.md` / `docs/architecture.md` | 移动端条目与仓库结构 | 已随本文件更新 |
| `tools/build-android.ps1` / `build-android.sh` | 一键构建 APK 并归置到 `out/`（默认 debug，`--release` 产出未签名包） | M1 已完成 |
| `docs/release.md` / CI | Android 构建暂不进 CI（同 iOS，需 SDK + 签名），本地提交前跑构建/lint/单测 | — |
| `hawk-daemon` | 已提供桌面 hawk-app 的 UDP 局域网发现；移动端自动发现与批量元数据端点（与 iOS 同步收益，M2 评估） | M2 |

## 风险

| 风险 | 影响 | 应对 |
| ---- | ---- | ---- |
| 明文 HTTP + 凭证落盘 | 同网段嗅探 / Root 设备读取 | 仅局域网允许明文；备份通道关闭；广域网强制 https；后续可加 Keystore |
| 广域网隧道带宽 | 自托管服务器流量成本 | 缩略图优先、预览才取原图、Coil 磁盘缓存 |
| 无统一端上 AI 能力 | M3 功能无法与 iOS 对齐 | 按设备能力检测降级；优先做「查询参数生成」这类可降级能力 |
| 大库性能未压测 | 万级素材首屏/滚动 | 与 iOS 同一套骨架 + 窗口分页；M2 做本地索引 |
| 厂商 ROM 后台限制 | 回前台重连延迟 | 只做前台刷新，不申请保活；错误可重试 |

## 待定项

1. **M2 索引存储**：Room 还是直接 SQLite（与 daemon 的 `index.db` 同语义的派生缓存），取决于是否需要 FTS
2. **系统集成**：Android 没有 Spotlight 的等价物，App Actions / 系统搜索索引的收益需要先验证
3. **端上 AI**：Gemini Nano 的设备覆盖有限，是否值得为少数机型做能力分支
4. **与 iOS 共享层**：等两端重复逻辑稳定后再评估 KMP

## 相关文档

- [架构设计](../architecture.md)
- [技术栈](../tech-stack.md)
- [iOS 客户端设计](ios-app.md)
- [hawk-app 设计](hawk-app.md)（桌面端与 web 查看器的交互语义）
- [REST API V1](../backend/server-rest-api-v1.md)
- [远程访问设计](../backend/remote-access.md) / [远程访问协议](../backend/remote-protocol.md)

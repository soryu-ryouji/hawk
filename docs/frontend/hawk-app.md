# hawk-app 设计（Electron 壳 + Vue 前端）

桌面版应用的设计文档。约束来源：[architecture.md](../architecture.md)（sidecar 进程模型）、[tech-stack.md](../tech-stack.md)（Vue 3 + TypeScript + Vite）、[server-rest-api-v1.md](../backend/server-rest-api-v1.md)（接口契约）。

## 目标

交付单机可用的素材管理桌面应用 v1，界面布局参考 Eagle，覆盖核心工作流：浏览、搜索、整理文件夹、标记素材（标签/评分/备注）、导入、回收站。

## Eagle 界面参考与取舍

Eagle 主窗口的关键特征：

```text
┌──────────┬────────────────────────────────────┬───────────────┐
│ 侧栏      │ 顶栏：侧栏开关·前进/后退·面包屑      │ 检查器         │
│ · 素材库  │  ‖ 缩略图滑杆 ‖ 筛选·搜索           │ · 大图预览     │
│ · 文件夹树├────────────────────────────────────┤ · 名称/评分    │
│ · 标签    │ 素材网格（瀑布流）                   │ · 标签/备注    │
│ · 回收站  │  · 缩略图卡片                       │ · URL/文件信息 │
│          │  · 多选 / 框选                       │               │
│          │  · 右键菜单                          │               │
│          │  · 双击放大预览                      │               │
└──────────┴────────────────────────────────────┴───────────────┘
```

左右栏通高不被顶栏隔断、顶栏只覆盖中间内容区、深色主题、无边框窗口 + 自绘顶栏（含窗口控制）、缩略图优先的网格浏览、右侧检查器即选即改，是 Eagle 体验的核心，全部采纳。

取舍：

| Eagle 特性 | 决策 | 说明 |
| ---------- | ---- | ---- |
| 三栏布局（左右栏通高，顶栏只覆盖中栏） | 采纳 | 侧栏/检查器色块通高到窗口上沿，不被顶栏隔断；中栏顶栏集成侧栏开关/前进后退/面包屑/缩略图滑杆/筛选/搜索；macOS 用系统原生红绿灯（`titleBarStyle: 'hidden'`，压在侧栏顶条左侧），Windows/Linux 无边框（`frame: false`）+ 自绘窗口控制（fixed 于窗口右上角）；不做状态栏 |
| 深色主题 | 采纳 | 自定义 CSS，不引组件库 |
| 瀑布流（不等高）网格 | 采纳（齐行布局） | Eagle 实为「行内等高、宽度按宽高比」的 justified 布局；自研贪心装行算法，不引库 |
| 侧栏标签云/智能文件夹 | 采纳（标签列表） | Category 维度落地后侧栏含分类列表与标签列表（见 category.md，分类为扁平受控词表）；智能文件夹不做 |
| 框选 | 不做 | 保留 Shift/Cmd 点选 |
| 评分筛选、颜色标签 | 评分筛选做（标题栏），颜色标签不做 | API 支持 star 过滤 |
| 浏览器扩展采集 | 已交付（hawk-browser-extension） | 独立 WXT 扩展，经 `GET /api/v1/app/token` 免配置接入；主界面不做导入入口，API 已就绪 |

## 进程模型与启动流程

严格遵循 architecture.md 的 sidecar 模式：前端不依赖 Electron IPC 做业务通信，主进程只管窗口、拉起/回收后端、注入 token。

```text
Electron 主进程启动
  → 读用户配置（userData/hawk-app.json）取最近素材库；无/失效 → 加载应用页（无连接参数，页面内进 SetupScreen）
  → 预选空闲环回端口（net.listen(0)）+ 生成随机 token，创建 BrowserWindow（show:false）
  → 立即加载应用页并注入 hash（开发：localhost:5173/#api=...&token=...；打包：file://index.html#...）——
    端口/token 先生成、页面先行，server 后台拉起；窗口内容单页生命周期，无二次导航，杜绝切换白屏
  → 首帧渲染完成后 ready-to-show 才 show 窗口（GPU 驱动不认可 backgroundColor、合成器首帧延迟时
    提前 show 会把空白/白窗暴露给用户）
  → spawn hawk-server（开发：dotnet 运行 dll；打包：process.resourcesPath 内二进制）
      环境变量传入 HAWK_TOKEN，参数 --library <path> --port <预选端口> --web-dist <web/dist>
      （--web-dist 供局域网 web 查看托管前端页面；asar 打包需 asarUnpack 该目录；
       缓存头：/assets/ 内容哈希资源 immutable 长缓存，index.html no-cache——防手机浏览器
       启发式缓存旧 HTML 引用已失效的旧 bundle，重建后手机端拿到构建前版本）
  → 轮询 GET /api/v1/app/startup（200ms，Bearer token；server 先监听、索引后台构建），事件推送页面：
      starting → hawk:server-progress（phase/processed/total，应用内启动屏呈现）
      ready    → hawk:server-started（含 address/token）→ 渲染进程重配 API（restart 会换端口）并 boot 数据
      error    → hawk:server-error（message 为后端给出的失败原因，页面内错误屏 + 退出入口）
    子进程异常退出 / spawn 失败 / 60s 超时 → hawk:server-error（stderr 尾部；有意 stopServer 除外）
退出
  → 杀掉子进程（含异常退出路径，防止孤儿进程）
```

启动/进度/错误全在单页内呈现（`useStartup` + `StartingScreen.vue`）：Electron 由主进程 IPC 推送，纯浏览器（局域网查看）无 IPC 则自行轮询 `/app/startup`（401 → ConnectScreen 门页）。换库（引导页选库）与应用设置重启 server 不再重载页面——`hawk:server-started` 带新地址/token 到达后渲染进程原地重配 API、重启数据（store.init + SSE 重连）。

握手全程走正规 HTTP（无任何 stdout 私有协议）：端口由主进程预选、token 由主进程生成，server 只负责绑定与构建索引；进度与就绪语义见 server-rest-api-v1.md「app/startup」。初始索引期间 `/api/*` 返回 503 `NOT_READY`（`app/startup` 除外），主界面只在 ready 后加载，因此前端无感。

关窗不退出（Eagle 式托盘驻留）：主进程拦截窗口 `close` 事件改为 `hide()`，
应用驻留系统托盘、`hawk-server` 保持运行（浏览器扩展采集不间断）。托盘左键单击
或菜单「打开 hawk」唤起主窗口，菜单「退出」才是真正退出（`before-quit` 置
`isQuitting` 放行 close 拦截，经 `will-quit` 回收 server）。再次启动应用由单实例锁
（`requestSingleInstanceLock`）转到已有实例——否则第二个实例的 hawk-server
会因 27371 端口占用直接启动失败。macOS 关窗后点 Dock 图标经 `activate` 唤起。
托盘为纯主进程行为，不新增 preload IPC 通道；自绘标题栏的关闭按钮与系统关窗
行为一致（同样落入托盘）。托盘图标复用 `build/icon.png`（512px 源图运行时按平台
重采样：Windows/Linux 32px、macOS 18px），该文件已列入 electron-builder `files`，
打包后可用。

token 经 URL hash 注入渲染进程（hash 不进 HTTP 请求、不进 History API），前端读取后保存在内存，不写 localStorage。

**preload 白名单**（contextBridge，只暴露这些，与业务数据无关）：

| 通道 | 用途 |
| ---- | ---- |
| `selectLibrary()` | 更换素材库：弹目录选择框 → 主进程杀掉旧 server 并用新库重启 → 重载窗口 |
| `showInFinder(path)` | 右键「在 Finder 中显示」，主进程 `shell.showItemInFolder` |
| `copyPath(path)` | 预览右键「复制文件路径」：主进程解析库内绝对路径后 `clipboard.writeText` |
| `copyImage(path)` | 预览右键「复制图片」：主进程 `nativeImage.createFromPath` + `clipboard.writeImage` |
| `getPathForFile(file)` | 拖拽导入时取文件绝对路径（Electron `webUtils`），供 `item/add` 使用 |
| `minimizeWindow()` / `toggleMaximizeWindow()` / `closeWindow()` | 自绘标题栏的窗口控制（仅 Windows/Linux；macOS 用系统原生红绿灯）；toggle 返回切换后的最大化状态 |
| `onWindowMaximized(cb)` | 订阅最大化状态变化（含 Aero Snap 等系统途径），标题栏据此切换 最大化/还原 图标；返回退订函数 |
| `onServerProgress(cb)` | 订阅后端扫描进度（`{ phase, processed, total }`，`total=0` 为不定态），应用内启动屏用；返回退订函数 |
| `onServerStarted(cb)` | 订阅 server 就绪：`{ address, token }`（冷启动/换库/应用设置重启都会到达；restart 会换端口，渲染进程须先重配 API 再重启数据）；返回退订函数 |
| `onServerError(cb)` | 订阅 server 启动/运行失败：`{ message }`（页面内错误屏呈现）；返回退订函数 |
| `quitApp()` | 真正退出应用（启动错误屏用；区别于 `closeWindow` 的隐藏到托盘） |

## 契约与类型生成

- 前端 TS 类型从 OpenAPI schema 生成（openapi-typescript），**不手写对接口**
- `npm run gen:types`：脚本启动 hawk-server（临时目录建库）→ 拉 `/openapi/v1.json` → 生成 `web/src/api/schema.d.ts` → 杀掉 server。生成文件入库，schema 变更时重新生成
- API client 统一处理：Bearer 头、信封解包（`status`/`data`/`error`）、错误码异常、SSE 用 `?token=`（EventSource 无法设请求头）

## 前端信息架构

### 布局

```text
┌────────────┬──────────────────────────────────┬─────────────────────┐
│ Sidebar    │ TitleBar                         │ Inspector           │
│  拖拽条¹[开关]│  [侧栏²][‹][›] 面包屑·已选N        │  拖拽条¹            │
│  库名 ⌄    │  [−滑杆＋]  [star▾][排序▾][🔍搜索]   │  预览图(格式角标)    │
│  全部素材 N ├──────────────────────────────────┤  名称(可改)          │
│  ───────── │  ItemGrid                        │  注释(可改)          │
│  ˅文件夹树  │   齐行网格：行内等高，宽度按宽高比    │  URL(可改)          │
│  (增/删/改) │   卡片下方：name.ext + 尺寸         │  标签 chips         │
│  ˅分类     │   虚拟渲染：只画视口±4行，离屏占位    │  分类 chips ＋      │
│  (增/删/改) │   懒加载 <img loading=lazy>        │  文件夹 chips ＋    │
│  ˅标签列表 N│   多选：Shift 连选 / Cmd 点选       │  基本信息(评分并入)  │
│  (增/删/改) │   双击/空格 → 预览浮层（Esc 关闭）   │  文件位置            │
│  回收站 N   │   右键：标签/分类/文件夹/回收        │                     │
│            │   拖入文件 → 导入                  │                     │
└────────────┴──────────────────────────────────┴─────────────────────┘
¹ 侧栏/检查器顶部各 40px 纯拖拽条（双击切换最大化）；macOS 原生红绿灯压在侧栏条左侧，
  Windows/Linux 自绘窗口控制 fixed 于窗口右上角（不随侧栏显隐移动）
² 侧栏开关：可见时在侧栏顶条右端；隐藏时挪到顶栏左上角
  文件夹/分类/标签三个分区标题可点击折叠/展开（v-show 保留树节点状态）
```

布局为 Eagle 式三栏：侧栏与检查器通高到窗口上沿，顶栏（`TitleBar.vue`）只覆盖中间内容区，顶部功能区按列分开不混在一起。窗口控制按平台区分：macOS 隐藏系统标题栏但保留原生红绿灯（`titleBarStyle: 'hidden'`，`trafficLightPosition` 按 40px 条高垂直居中，悬停 glyph/失焦置灰/全屏行为由系统保证）；Windows/Linux 为无边框窗口（`frame: false`），自绘窗口控制 fixed 在窗口右上角，经 preload 白名单 IPC 驱动主进程。三条顶条均为窗口拖拽区（双击空白切换最大化），顶栏交互控件单独 `no-drag`；侧栏隐藏时顶栏通栏：macOS 左端预留 78px 避让原生红绿灯，Windows/Linux 右端预留 130px 避让 fixed 的窗口控制。侧栏与检查器宽度可拖拽调整：栏分界线上紧贴右侧压 4px 命中区手柄（避开左侧面板右缘的纵向滚动条，防止拖滚动条误触调宽；App.vue 内联 style 控制 grid 列宽，拖拽期间 body 锁定 col-resize 光标并禁用文本选择），侧栏 180–480px、检查器 240–560px，宽度持久化到 localStorage（`hawk:panelWidths`，全局生效不随素材库变）。无独立状态栏：计数在侧栏各行徽章（全部素材/根目录素材/未分类素材/未标签素材/文件夹/分类/标签/回收站），选中数在顶栏面包屑旁。侧栏行首为描边小图标（Icon.vue，feather 风格 inline SVG）。

**移动端竖屏适配**（断点 `max-width: 720px`，`useIsMobile` 判定并同步 `body.mobile`，局域网 web 查看的手机场景）：`.app` 改单栏（`1fr`），侧栏变**抽屉**（fixed + `translateX(-105%)` 滑出，`drawer-open` 滑入；`no-panels` 在移动端不 display:none 侧栏——显隐由 transform 管，否则滑入滑出动画被 display 切换扼杀），汉堡按钮在顶栏左上角（复用侧栏开关），点导航项/遮罩自动收起；检查器与栏宽拖拽手柄隐藏，顶栏隐藏缩略图尺寸组/排序/评分筛选/选中计数、压缩搜索宽度（保留汉堡/前进后退/搜索/设置，390px 实测防横向溢出）；**触屏无双击**：点按卡片直接开预览（ItemGrid onSelect 移动端分支）；**网格行宽硬顶**：齐行布局的行高夹紧（0.5×–1.75×目标高）与末行规则不得把行推出容器——行高最终以容器宽反推值（fitH）为硬顶，桌面容器宽极少生效，移动端窄屏遇全景图等宽行时杜绝图片出屏；**预览全沉浸**：图片占满 100vw/100vh，关闭 × 与底部翻页栏隐藏——横向滑动切换上一张/下一张（传送带动效，相邻原图预加载免解码闪烁），**下拉关闭**（阻尼跟手 + 背景随位移渐亮，≥96px 松手下滑出关闭，桌面保留 ×/Esc/点遮罩），右上角 **ⓘ 开关底部详情条**（检查器在移动端无入口，预览内只读查看名称/尺寸/大小/评分/标签/分类/文件夹/修改时间/注释，条内独立滚动与下拉关闭手势隔离）；**编辑窗口旋转约束互换**：90/270° 旋转后视觉宽高互换，`max-width`/`max-height` 约束同步互换，否则竖屏下旋转长边水平出屏；编辑窗口底栏按钮加大命中区（`body.mobile` 命中，浮层 Teleport 到 body 不在 `.app` 内）。桌面布局与交互完全不变。

网格为**齐行布局**（justified layout，与 Eagle 一致）：贪心装行，非末行按容器宽精确反推行高，单元格与图片同宽高比——图片完整显示不裁切。由 ItemGrid 按宽高比计算 flex 行（ResizeObserver 驱动），非 CSS grid。**虚拟渲染（Eagle 式滚动条）**：store 持当前视图全量骨架（`skeleton`：id/width/height/star，经 `item/skeleton` 一次性取回，与 `item/list` 同查询同排序且主键同值时按 id 打破平局——两次查询次序逐位一致），ItemGrid 用骨架算出全部行的 y 偏移，容器总高即时确定，滚动条可自由拖动跳转；只渲染视口 ±4 行（绝对定位 + translateY），行内详情未拉取的单元格只留宽高的占位块（不渲染图片），进入视口时按骨架索引区间经 `ensureWindow` 用 `item/list` 补数据。多选面板（Inspector）提供批量添加标签/分类/移动文件夹、批量评分、总大小与堆叠预览。

当前位置体现在侧栏选中态与标题栏：文件夹视图渲染可点击面包屑（根 = 全部素材，逐级跳转），其余视图为固定标题；标题栏前进/后退在会话内浏览历史（`setView` 压栈，重命名跟随/删除回退就地修正当前条目）中移动。回收站视图下右键菜单变为「恢复 / 彻底删除（清空回收站，二次确认）」。文件夹树节点计数由后端 `folder/list` 的 `count` 字段提供（含子级、不含回收站、按 item 去重）；分类/标签计数由 `category/list`、`tag/list` 提供（精确匹配、不含回收站）。

### 目录结构（web/src）

```text
web/
├── index.html
├── tsconfig.json
└── src/
    ├── main.ts                # 入口：解析 hash 注入 api/token、创建 Pinia、挂载 App
    ├── App.vue                # 布局骨架；挂载全局 composables（快捷键/拖拽导入）与浮层
    ├── types.ts               # 业务类型（ViewState/QueryState/MenuItem）；Item 等从 schema.d.ts 别名导出
    ├── dnd.ts                 # 素材拖拽共享工具（网格→侧栏）：ITEMS_MIME、startItemsDrag/itemsDragOver/readItemsDrop
    ├── api/
    │   ├── client.ts          # request 封装：baseURL、Bearer 头、信封解包、ApiError
    │   ├── endpoints.ts       # 全部端点的强类型函数（api.itemList(...) 等）
    │   ├── events.ts          # SSE 连接管理：订阅、重连、分发
    │   └── schema.d.ts        # openapi-typescript 生成（入库，勿手改）
    ├── stores/
    │   └── library.ts         # Pinia 主 store（见下）
    ├── composables/           # 只放业务 composable；通用能力直接用 @vueuse/core
    │   ├── useContextMenu.ts  # 右键菜单状态（visible/x/y/items）
    │   ├── useDragImport.ts   # 拖拽导入（文件夹递归展开 + 对接 store.importPaths）
    │   └── useShortcuts.ts    # 全局快捷键映射（内部基于 VueUse useEventListener）
    │   └── useGridNav.ts      # 网格选中框空间导航（ItemGrid 发布行布局，方向键消费）
    │   └── useIsMobile.ts     # 移动端判定（matchMedia ≤720px，同步 body.mobile）：驱动抽屉侧栏/点按开预览/顶栏减负
    │   └── useStartup.ts      # 启动状态机：server-started/error/progress 事件（Electron IPC）或浏览器轮询 /app startup，就绪计数驱动 App (re)boot
    ├── components/
    │   ├── TitleBar.vue
    │   ├── WindowControls.vue
    │   ├── Icon.vue           # 描边小图标（feather 风格 inline SVG，侧栏/按钮行首）
    │   ├── SetupScreen.vue    # 引导页：选库（Electron 内素材库未配置），选定后经 server-started 事件进启动屏
    │   ├── StartingScreen.vue # 应用内启动屏：server 扫描索引期间的进度反馈（替代旧独立 loading.html，单页生命周期无切换白屏）
    │   ├── ConnectScreen.vue  # 连接门页：局域网 web 查看先输入 token，验证通过后记忆、再访问免输入直连
    │   ├── Sidebar.vue
    │   ├── FolderTreeNode.vue
    │   ├── ItemGrid.vue
    │   ├── ItemCard.vue
    │   ├── Inspector.vue
    │   ├── TagEditor.vue      # 标签 chip 编辑器（Inspector 的子组件）
    │   ├── CategoryPickerDialog.vue # 分类选择模态（可选已有，也可输入新名字）
    │   ├── StarRating.vue     # 点星评分（Inspector/右键菜单共用）
    │   ├── PromptDialog.vue   # 文本输入模态（添加标签/新建文件夹）
    │   ├── FolderPickerDialog.vue # 文件夹选择模态（移动到文件夹）
    │   ├── PreviewOverlay.vue
    │   ├── ImageEditDialog.vue # 图片编辑窗口（右键「编辑图片…」）：旋转预览 + 保存/放弃/取消三选确认
    │   ├── ContextMenu.vue    # 全局单例自绘菜单
    │   └── EmptyState.vue     # 空库/空结果占位
    └── styles.css             # 深色主题 CSS 变量与全局样式
```

界面文案中文硬编码，v1 不做 i18n。

### 类型层（types.ts）

```ts
import type { components } from './api/schema';

// 契约类型一律从生成的 schema 取，不另写
export type Item = components['schemas']['ItemDto'];
export type FolderNode = components['schemas']['FolderNode'];
export type LibraryInfo = components['schemas']['LibraryInfo'];

// 业务自有类型
export type ViewState =
  | { kind: 'all' }
  | { kind: 'root' }          // 根目录素材（直接位于库根，不含子文件夹）
  | { kind: 'uncategorized' } // 未分类素材
  | { kind: 'untagged' }      // 未标签素材
  | { kind: 'folder'; path: string }
  | { kind: 'category'; name: string }
  | { kind: 'tag'; name: string }
  | { kind: 'trash' };
export interface QueryState {
  keywords: string[];
  star?: number;
  color?: string;   // 颜色检索（#hex）；Inspector 调色板点击设置/再点当前色清除
  orderBy: 'modification_time' | 'name' | 'size' | 'star';
  order: 'asc' | 'desc';
}
export interface MenuItem { label: string; danger?: boolean; separator?: boolean; action?: () => void }
```

### API 层

**client.ts**——模块级单例，启动时解析连接参数（桌面端经 hash 注入；局域网 web 查看走同源回退 + token 门页）：

```ts
export class ApiError extends Error {
  constructor(public code: string, message: string, public httpStatus: number) { super(message); }
}
export function initApi(): ApiConfig | null
// api 解析优先级：location.hash(#api=…) > VITE_HAWK_API(纯前端调试) > 同源 location.origin(仅纯浏览器:
// 页面由 hawk-server 托管的局域网 web 查看场景;Electron 内无 hash 视为未配置)
// token 解析优先级：hash > ?token= 查询参数 > localStorage(键 hawk:token:<host>,ConnectScreen 验证通过后写入,
// 同一服务端再访问免输入直连)
export function storeToken(api, token) / clearStoredToken(api): void  // localStorage 按 api host 隔离
export function setApiToken(token): void  // ConnectScreen 验证通过后更新当前连接
export async function request<T>(method: string, path: string,
  opts?: { body?: unknown; query?: Record<string, string> }): Promise<T>
// 行为：拼 Bearer 头；信封解包（status==='error' → throw ApiError）；网络错误 → ApiError('NETWORK')；无 data → undefined
```

**局域网 web 查看**（server 侧见 storage.md 的 `[web]` 段与 server-rest-api-v1.md 的 `app/info.access`）：桌面端设置面板配置 enabled/port/token（按库隔离），server 追加监听 `0.0.0.0:<port>` 并托管前端静态文件；浏览器打开 `http://<电脑IP>:<port>` 先进 ConnectScreen 输入 token，验证通过后经 `app/info.access` 进入 viewer/admin 模式——**viewer 为只读**：前端隐藏全部写入口（store.viewerMode 驱动：右键写菜单/编辑窗口/检查器编辑字段/侧栏新建/多选批量/删除快捷键/拖拽导入与放置），服务端写端点对该 token 一律 `403 READ_ONLY` 为最终防线。

**endpoints.ts**——端点一一对应 server-rest-api-v1.md，签名即契约：

```ts
export const api = {
  appInfo(): Promise<AppInfo>;
  libraryInfo(): Promise<LibraryInfo>;
  reindex(): Promise<void>;
  folderList(): Promise<FolderNode>;
  folderCreate(name: string, parentPath?: string): Promise<FolderNode>;
  folderUpdate(path: string, patch: { name?: string; parent_path?: string }): Promise<FolderNode>;
  folderDelete(path: string): Promise<void>;
  folderRestore(path: string): Promise<void>;
  itemList(params: ItemListParams): Promise<{ items: Item[]; total: number; offset: number; limit: number }>;
  itemSkeleton(params): Promise<{ items: SkeletonItem[]; total_size: number }>;  // 全量骨架：同过滤同排序（确定性次序）、不分页，虚拟网格建完整布局用
  itemDetail(id: string): Promise<Item>;
  itemCount(): Promise<number>;
  itemAddByPath(path: string, opts?: { name?: string; folder_path?: string; tags?: string[] }): Promise<{ item: Item; already_existed: boolean }>;
  itemUpdate(id: string, patch: { name?; tags?; categories?; star?; annotation?; url?; folder_path? }, path?: string): Promise<Item>;
  itemBatchUpdate(ids: string[], patch: { add_tags?; add_categories?; star?; folder_path? }): Promise<{ updated: number; missing_ids: string[] }>;  // 批量：标签/分类并集、评分/文件夹设置（见 API 文档 batch_update 节）
  itemDelete(id: string, path?: string): Promise<void>;
  itemRestore(id: string, path?: string): Promise<void>;
  refreshThumbnail(id: string): Promise<void>;
  itemReplace(id: string, imgBase64: string, path?: string): Promise<Item>;  // 客户端编辑后的内容替换；哈希变化 → id 漂移，响应为新 Item（新 id）
  trashClear(): Promise<void>;
  // category / tag 注册表端点（见 server-rest-api-v1.md 与 category.md）
  categoryList(): Promise<CategoryInfo[]>;                        // [{ name, count }]
  categoryCreate(name: string): Promise<void>;
  categoryUpdate(name: string, newName: string): Promise<void>;   // 重命名，目标已存在时合并
  categoryDelete(name: string): Promise<void>;
  tagList(): Promise<TagInfo[]>;                                  // [{ name, count }]
  tagCreate(name: string): Promise<void>;
  tagUpdate(name: string, newName: string): Promise<void>;        // 重命名，全部 item 跟随
  tagDelete(name: string): Promise<void>;
  thumbnailUrl(id: string, size?: number): string;  // 拼 ?token= 的 <img> URL；size 须命中服务端 thumbnail_sizes 白名单
  fileUrl(id: string): string;  // 原图 URL（预览浮层用），同样拼 ?token=
};
```

**缩略图与原图的鉴权**：`<img>` 无法带请求头，采用 `?token=` 查询参数。需后端配合：`TokenAuthMiddleware` 对 `GET /api/v1/item/thumbnail`、`GET /api/v1/item/file` 放行查询参数 token（与 events 同款）。缩略图与原图 URL 因此稳定，配合 `Cache-Control: immutable` 获得浏览器级缓存。检查器 1024 大图同理。

**events.ts**：

```ts
export function connectEvents(handlers: {
  onAdded(item: Item): void; onUpdated(item: Item): void;
  onTrashed(id: string): void; onRestored(item: Item): void; onRemoved(id: string): void;
  onTaskProgress(p: { task: string; pending: number; active: number }): void;
  onFolderChanged(reason: string): void;   // 目录结构变化：重拉 folder/list（reason 恒为 external，忽略取值）
  onReconnect(): void;   // EventSource 断线重连成功后全量对齐
}): () => void;           // 返回断开函数（App 卸载/换库时调）
```

事件名与负载的字段契约以 server-rest-api-v1.md「events」节为准（`ItemEvents` 常量与文档一一对应），此处只做类型分发，不自定义负载形状。

### Pinia store（stores/library.ts）

单一 store `useLibraryStore`，组件不直接调 api（除缩略图 URL 拼接），一切经 action：

```ts
// ---- state ----
view: ViewState;                 // 默认 all
query: QueryState;               // 默认 { keywords: [], orderBy: 'modification_time', order: 'desc' }
skeleton: SkeletonItem[];        // 当前视图全量骨架（item/skeleton 一次取回）：布局与滚动条总高的唯一依据
details: Map<string, Item>;      // 已拉取详情（视口窗口 + 预取），按 id 索引
total: number;                   // = skeleton.length
totalSize: number; loading: boolean; windowLoading: boolean;   // 整表加载中 / 视口窗口补数据中
selection: string[];             // 选中 id，有序；末位为主选中/连选锚点（selectAll 基于全量骨架）
folders: FolderNode | null;      // 完整树（含根）
library: LibraryInfo | null;
thumbSize: number;               // 网格卡片边长偏好（默认 160，内存态不持久化）
sidebarVisible: boolean;         // 侧栏显隐（标题栏开关，默认开）
previewId: string | null;        // 预览浮层
// 缩略图后台积压（task.progress 驱动；null 无积压，App.vue 顶部细进度条据此显隐）
taskBacklog: { pending: number; active: number } | null;
toast: string | null;            // 轻提示（3s 自动清除）
importProgress: { total: number; done: number } | null;  // 导入进度：null 无任务；total=0 收集文件阶段（不定态进度条）
// 会话内浏览历史：viewHistory/historyIndex，setView 压栈，数据变更修正就地替换当前条目

// ---- getters ----
isTrash: boolean;                // view.kind === 'trash'
currentFolderPath: string | null;
selectedItems: Item[];
primarySelected: Item | null;    // selection 末位对应的 item
previewItem / previewPrevId / previewNextId: 浮层与左右切换
thumbSizes: number[];             // library/info 的 thumbnail_sizes 升序（网格 img srcset 候选；缺字段兜底 [256, 1024]）

// ---- actions ----
init(): Promise<void>;           // libraryInfo + folders + resetList；失败进启动失败态
setView(v: ViewState): void;                     // 切视图：压浏览历史 → 清空选择 → resetList
goBack() / goForward(): void;                    // 标题栏前进/后退（canGoBack/canGoForward 驱动禁用态）
toggleSidebar(): void;                           // 侧栏显隐开关
setQuery(patch: Partial<QueryState>): void;        // → resetList
resetList(): Promise<void>;      // 取全量骨架（skeletonVersion 作废过期响应）→ 清 details → ensureWindow(0, 150)
ensureWindow(start, end): Promise<void>;  // 视口窗口补数据：按骨架索引区间拉 item/list（次序逐位对齐），已缓存则跳过
reloadSkeleton(): Promise<void>; // SSE 驱动骨架重载：成员/次序以服务端为准；滚动位置不动，详情缓存保留并清理失效项
select(id: string, mod?: 'range' | 'toggle'): void;
selectAll(): void; clearSelection(): void;
updateItem(id: string, patch): Promise<void>;      // 就地更新 items；ApiError → toast
addCategoryToSelected(name) / addTagToSelected(tag): Promise<void>;  // 批量端点并集追加（已含该分类/标签的项跳过）；ensureSelectionDetails 先补齐选中项详情
moveSelectedToFolder(path): Promise<void>;         // 批量端点移动主位置；已在目标处的项跳过
setStarForSelected(star): Promise<void>;           // 批量端点设置评分（多选面板与右键菜单共用）
batchUpdate(ids, patch, doneText): Promise<void>;  // 批量端点统一入口；missing_ids 计数在 toast 提示「n 个未处理」
trashSelected(): Promise<void>; restoreSelected(): Promise<void>;
clearTrash(): Promise<void>;                       // 调用方先二次确认
importPaths(paths: string[]): Promise<void>;       // 逐个 itemAddByPath（server 逐文件处理完才返回，done 逐项推进）；结束汇总 toast（成功 n，已存在 m，失败 k）
importBegin(): boolean;                           // 拖拽落下即占用导入态（并发导入拒绝并 toast）；importPaths 前置
refreshFolders(): Promise<void>;
openPreview(id): void; closePreview(): void; navigatePreview(step: 1 | -1): void;
editorTarget: Item | null;   // 图片编辑窗口目标(全局单例,App.vue 据此挂载 ImageEditDialog)
viewerMode: boolean;          // 局域网 viewer token(只读查看):隐藏全部写入口,服务端 403 为最终防线
openEditor(item): void; closeEditor(): void;   // 网格/预览浮层右键「编辑图片…」
saveImageEdit(id, angle: 90 | 180 | 270): Promise<boolean>;  // 编辑窗口保存：客户端重编码（canvas，JPEG EXIF 字节级回填并重置 Orientation，见 imageEdit.ts）→ item/replace；id 漂移后新 item 就地替换详情（预览若正打开则跟随新 id）；返回是否成功，调用方据此关闭编辑窗口
showToast(msg: string): void;
applyEvent(type: string, payload: unknown): void;  // SSE 分发入口（策略见下节）
```

### Vue 实践基线

- SFC 一律 `<script setup lang="ts">`；props/emits 用类型式声明（`defineProps<{...}>()` / `defineEmits<{...}>()`）
- 组件样式一律 `<style scoped>`；全局样式只有 styles.css 的变量与 reset
- 浮层类（PreviewOverlay/ContextMenu/toast）经 `<Teleport to="body">` 挂载，避免层叠上下文与 overflow 裁切
- 列表 `v-for` 必须绑定 `:key="item.id"`
- 全局监听/观察器一律走 @vueuse/core（随组件卸载自动清理），不手写 `addEventListener`
- 组件不直接调 api，一切经 Pinia action；跨组件共享逻辑才抽 composable
- 组件局部状态用 `ref`；items 大数组只做整体替换，不依赖深度响应式
- 构建先过类型检查：`vue-tsc --noEmit && vite build`

### 组件契约

| 组件 | props | emits | 职责与内部状态 |
| ---- | ----- | ----- | -------------- |
| `App.vue` | — | — | 布局骨架（侧栏/检查器通高两行、顶栏只占中栏；`no-panels` 时左右两栏同时归零，Eagle 式侧栏开关；栏宽拖拽手柄，内联 style 控制 grid 列宽，宽度持久化 `hawk:panelWidths`；WindowControls fixed 于窗口右上角）；**启动阶段状态机**（`phase`: starting/ready/setup/connect/error）：starting 显示应用内启动屏，`useStartup` 就绪计数（Electron 经 `server-started` IPC 且先重配 API；浏览器轮询 /app/startup）触发 runBoot——store.init（401 → 清残留 token 进门页）+ connectEvents（SSE 先断后连，restart 换地址后重挂）；server-error 进错误屏（退出入口）；setup=引导页选库（选定转 starting）、connect=浏览器 token 门页；窗口内容单页生命周期，无 hashchange/二次导航；挂载全局快捷键/拖拽 composable；挂载 PreviewOverlay/ImageEditDialog（store.editorTarget 驱动）/SettingsDialog/ContextMenu/toast/导入进度浮层（底部居中，收集文件不定态 → 逐项推进，与 toast 同层叠放并避让）；前置阶段（启动/引导/门页/错误）带拖拽条与窗口控制 |
| `TitleBar.vue` | — | `open-settings` | Eagle 式中栏顶栏（只覆盖内容区，窗口拖拽区，双击空白切换最大化）：侧栏开关（仅侧栏隐藏时在本栏左上角；可见时开关在侧栏顶条右端）、前进/后退、位置面包屑（文件夹/分类逐级跳转）+ 选中计数、缩略图滑杆（−/＋步进）、读写 store.query（搜索框回车按空格拆 keywords、star 筛选下拉、颜色筛选 chip、排序下拉）；右端设置齿轮（仅 Electron，打开 SettingsDialog）；侧栏隐藏时通栏，macOS 左端预留避让原生红绿灯、Windows/Linux 右端预留避让 fixed 窗口控制 |
| `WindowControls.vue` | — | — | 最小化/最大化(还原)/关闭按钮（Windows/Linux 风格），fixed 于窗口右上角（z-index 100，预览浮层/对话框之下），侧栏显隐不影响位置；macOS 不渲染（系统原生红绿灯）；控件区由本组件自带 `app-region: no-drag`（下方是拖拽区，缺了真实点击会被拦截）；仅 Electron 内渲染；最大化态经 `onWindowMaximized` 订阅同步 |
| `Icon.vue` | `name: IconName`、`size?: number`（默认 15） | — | 描边小图标（feather 风格 inline SVG），侧栏行首/按钮图标统一入口；name 为内置图标名联合类型 |
| `SetupScreen.vue` | — | `selected` | 引导页：Electron 内素材库未配置/失效时展示，经 preload `selectLibrary()` 选库（主进程即生成端口/token 拉起 server），返回 true 发 `selected` 切启动屏，就绪经 `server-started` 事件进主界面；spawn 失败主进程弹系统框并留本页 |
| `ConnectScreen.vue` | — | `connect` | 局域网 web 查看连接门页：输入 token → `setApiToken` 后经 `app/info` 验证（401 → 「token 无效」），通过则 `storeToken` 按 api host 记入 localStorage 并 `emit('connect')` 重新 boot——之后访问同一服务端免输入直连 |
| `SettingsDialog.vue` | — | `close` | 设置面板（仅桌面端，TitleBar 齿轮打开）：局域网查看开关/端口/访问 token（重新生成随机串）/本机局域网地址列表；按库隔离存于 `.hawk/config.toml` 的 `[web]` 段，保存经 preload `saveLanSettings()` 由主进程写配置并重启 hawk-server（await ready 以支持失败自动回滚并弹错）；成功不重启页面——server-started 事件驱动 App 原地换地址重载数据，本对话框 emit close |
| `Sidebar.vue` | — | — | 顶部 40px 拖拽条（macOS 红绿灯压在其左侧，右端为侧栏开关），内容区独立滚动：智能条目（全部素材/根目录素材/未分类素材/未标签素材/回收站，各带计数，Eagle 式置顶）→ 文件夹/分类/标签分区（标题点击折叠/展开，v-show 保留树节点状态；标签行左缩进与树节点名称列对齐）；底部固定区为设置按钮（设置面板接入前 toast 占位），不随列表滚动；选中态反映 store.view；分类/标签容器接受素材拖入（容器级委托 + 行高亮，drop → 添加分类/标签） |
| `FolderTreeNode.vue` | `node: FolderNode`、`depth: number` | — | 内部态：expanded、editing（重命名/新建的内联 input）、dropDepth（素材拖入高亮计数）；点击 setView；右键菜单：新建子文件夹/重命名/删除（确认）；**接受素材拖入**（drop → `moveSelectedToFolder(node.path)`，悬停高亮） |
| `ItemGrid.vue` | — | — | 齐行布局 + 虚拟渲染：骨架算全量行 y 偏移（总高即时确定，滚动条可自由拖动），scroll rAF 驱动可见区间（±4 行 overscan，绝对定位 translateY），行内详情经 store.ensureWindow 补齐、未到位时占位块只留宽高；空态 EmptyState；右键/双击/点选转发 store。右键菜单：添加标签/添加到分类/移动到文件夹/编辑图片（仅 canvas 可重编码的 jpg/png/webp，`store.openEditor(item)`，编辑对象 = 右键点击的那张，与多选无关）/在文件管理器中显示/评分/移入回收站；菜单触发的选择器对话框（PromptDialog/CategoryPickerDialog/FolderPickerDialog）就地挂载在本组件 |
| `ItemCard.vue` | `item: Item`、`selected: boolean`、`size: number` | `select(id, MouseEvent)`、`open(id)`、`menu(id, x, y)` | 缩略图（`loading=lazy`，加载失败显示 ext 占位块）、名称、★ 角标；可拖拽（`draggable`，回收站禁用）：拖未选中项改为单选它、拖已选中项带动整个选择集，dragstart 写 `application/x-hawk-items` 供侧栏放置 |
| `Inspector.vue` | — | — | 顶部 40px 拖拽条（Windows/Linux 的窗口控制 fixed 在其右侧），内容区独立滚动。单选：1024 预览 + 调色板色块行（点击在当前视图范围内按颜色检索，再点当前色清除）+ 可编辑字段（失焦提交 updateItem；名称/注释为自动增高 textarea，名称回车提交且换行转空格，注释支持多行、Ctrl+Enter 提交）；多选：数量 + 批量按钮；只读信息区（ext/尺寸/大小/mtime/id 短码/全部路径）；无选中：当前分区状态（视图名 + 文件数/占用空间，取自 item/list 的 total/total_size） |
| `TagEditor.vue` | `modelValue: string[]` | `update:modelValue` | chip + 删除；「＋」按钮展开内联输入（带既有标签候选 datalist），Enter/失焦提交、Esc 取消（trim 去重） |
| `CategoryPickerDialog.vue` | `title: string` | `confirm(name: string)`、`cancel` | 分类输入模态：输入框带已有分类候选（datalist），可输入新名字；确认单个分类名（Inspector「＋添加到分类」与多选批量添加共用） |
| `StarRating.vue` | `modelValue: number` | `update:modelValue` | 5 星；点当前星值 → 清零 |
| `PromptDialog.vue` | `title, placeholder?` | `confirm(value)`、`cancel` | 通用文本输入模态（Enter 提交/Esc 取消） |
| `FolderPickerDialog.vue` | `title` | `confirm(path)`、`cancel` | 文件夹选择模态（扁平树下拉） |
| `PreviewOverlay.vue` | `item: Item` | `close`、`navigate(1\|-1)` | 全屏展示原图（`/item/file`）；Eagle 式磨砂玻璃遮罩覆盖底层界面，右上角 × 关闭；滚轮以光标为不动点缩放、双击复位；**手势两级语义**：缩放>1 单图平移模式（v-if 互斥，缩放=1 为 carousel 模式）；carousel = **三图轨道**（前|当前|后 并排，iOS 相册式）：横向拖动时左右邻图实时可见，过 56px 阈值松手邻图滑至屏幕中央（轨道动画结束才提交切换并无缝复位，配合相邻原图 `new Image()` 预加载免解码等待），首/末张边缘橡皮筋阻尼（0.35x），不足阈值回弹；**手势层与位移层分离**（`.swipe-track` 固定全屏承接 pointer 事件，内层 `.track-row` 承载 transform——位移会改变元素命中区域，同层会导致按下即丢失命中）；移动端另支持**下拉关闭**（阻尼跟手+背景渐亮，≥96px 松手滑出关闭；桌面下拉不触发，保留 ×/Esc/点遮罩）与**ⓘ 底部详情条**（只读展示当前项元信息，补检查器在移动端的缺位）；`previewItem` 为 sticky（详情未加载不置空，防浮层卸载重建）；Esc/点遮罩/空格关闭；←/→ 或底部按钮切换（**移动端隐藏翻页栏与 ×**）；右键菜单：在文件管理器中显示/复制文件路径/复制图片/编辑图片（仅 jpg/png/webp，`store.openEditor`，保存后本浮层经 previewId 切换到新 id 显示旋转结果；放弃则保持原图）/删除图片（删除后跳到下一张，末张关闭） |
| `ImageEditDialog.vue` | `item: Item` | `close` | 图片编辑窗口：全屏 Eagle 式遮罩（观感同预览浮层、层级高于它），底部中间工具条为 ↺/↻ 旋转 + 「已旋转 n°」+ 退出/保存；`store.editorTarget` 驱动、App.vue 全局挂载（网格与预览浮层右键「编辑图片…」均可打开）。编辑期间旋转只作用于预览角（CSS 变换）；「保存」或带修改退出（×/退出/Esc/点遮罩）时三选确认（保存/不保存/取消）才经 `store.saveImageEdit` 做客户端重编码（canvas，EXIF 方向烘焙进像素；JPEG EXIF 字节级回填、Orientation 重置为 1）并提交 `item/replace`；id 漂移后详情就地替换、预览若正打开则跟随新 id；写回保留原修改时间，素材在按时间排序中不挪位 |
| `ContextMenu.vue` | — | — | 读 useContextMenu 状态渲染；点外部/Esc 关闭 |
| `EmptyState.vue` | `text: string` | — | 空态文案与「拖入文件开始」提示 |

### composables

通用能力不重复造：滚动驱动用原生 `@scroll` + rAF（可见区间计算在 ItemGrid.vue 内联，配合骨架布局做虚拟渲染）；拖拽用 `useDropZone`；全局监听用 `useEventListener`。业务 composable 只保留三个：

| composable | 签名与行为 |
| ---------- | ---------- |
| `useContextMenu()` | 模块级单例响应式状态 `{visible, x, y, items}`（全局唯一菜单）；`open(items, MouseEvent)` 定位（防出屏翻转）；`close()` |
| `useDragImport()` | `useDropZone` 接 drop → 先 `importBegin()` 占位（收集文件阶段进度条即显示）→ `webkitGetAsEntry()` 递归展开文件夹 → `webUtils.getPathForFile` 取绝对路径 → `store.importPaths`；收集失败 toast |
| `useShortcuts()` | 全局 keydown：焦点在 input/textarea 时跳过；**图片编辑窗口打开时（store.editorTarget）整体让行**（窗口自带 Esc/关闭逻辑，否则 Esc 会关底层预览、Delete 会删正在编辑的素材）；`Delete/Backspace` → 按视图 trashSelected/restoreSelected；`Esc` → 关浮层/菜单；`Cmd/Ctrl+A` → selectAll；`←/→`（浮层打开时）→ navigatePreview。另有 main.ts 的捕获阶段拦截：IME 组合态（中文输入法选词）中的 Enter/Escape 不下发——Enter 是确认候选而非提交，Esc 是关候选窗而非取消 |

### 样式约定

深色主题，CSS 变量集中 `styles.css :root`：

```css
--bg-0: #1e1e1e;  /* 主区 */  --bg-1: #252526;  /* 侧栏 */  --bg-2: #2d2d30;  /* 检查器/卡片 */
--fg-0: #e8e8e8;  --fg-1: #9d9d9d;  --accent: #4f8cff;  --danger: #e5534b;  --border: #3c3c3c;
```

布局用 CSS Grid：`220px 1fr 280px` × `40px 1fr`，侧栏/检查器跨两行通高，顶栏只占中栏首行；侧栏可经顶栏开关隐藏归零（`no-panels` 时顶栏通栏）；窗口控制 fixed 于窗口右上角不占 grid。网格卡片 `repeat(auto-fill, minmax(var(--thumb-size), 1fr))`，卡片内缩略图定高 + `object-fit: contain`。

### 错误处理

ApiError 统一在 store action 捕获 → `showToast`（错误码 → 中文文案映射：`FILE_EXISTS`→「同名文件已存在」、`ITEM_NOT_FOUND`→「素材不存在或已被移除」……其余透传 message）。toast 固定底部居中，3s 自动消失。启动级失败（无 token / 连不上 server）渲染整页错误态而非 toast。

### SSE 增量刷新策略

订阅 `/api/v1/events`，按事件类型处理：

| 事件 | 处理 |
| ---- | ---- |
| `item.updated` | 负载是完整 Item。详情在缓存中就地替换立即反映；骨架上的 star 同步（★ 角标）。过滤视图/激活查询条件时防抖 200ms 重载骨架（成员判定以服务端查询为准，如摘掉当前分类后 item 即时消失）。updateItem 响应走同一入口 |
| `item.added` / `item.restored` | 新 item 落点（成员/次序）以服务端为准，防抖 200ms 重载骨架 |
| `item.trashed` / `item.removed` | 就地移除（详情 + 骨架 + 选择），回收站视图同事件意味着「进来」，统一防抖重载兜底 |
| `task.progress` | 更新 `taskBacklog`（缩略图积压计数；归零置 null 隐藏指示条），不触发文件夹/分类刷新 |
| `folder.changed` | 防抖重拉文件夹树（reason 恒为 external，忽略取值）；骨架成员与分类/标签计数无关，不触发 |
| 任何 item 事件 | 防抖刷新文件夹树与分类/标签计数（folder.changed 已覆盖目录结构变化，此项兜底文件夹内 item 计数变动） |

断线自动重连（EventSource 原生行为），重连后 reloadSkeleton + refreshFolders 全量对齐。

## 功能清单 v1（验收标准）

1. 启动选库：首次启动弹目录选择；记住上次素材库与上次浏览的文件夹视图（按库路径存 localStorage，文件夹已删则回退全部素材）；菜单可更换库
2. 侧栏：智能条目（全部素材/根目录素材/未分类素材/未标签素材/回收站）/ 文件夹树 / 分类列表 / 标签列表（三分区均支持「＋」新建与右键重命名/删除；**接受素材拖入**：拖到文件夹=移动，拖到分类/标签=添加）
3. 网格：缩略图懒加载、虚拟渲染（打开即知总高、滚动条自由拖动、离屏不渲染）、单选/Shift 连选/Cmd 点选、双击预览浮层
4. 搜索与筛选：关键词（命中名称/备注）、star 精确筛选、四种排序双向
5. 检查器：1024 预览；名称、标签（chip 增删）、评分（点星）、备注、URL 编辑即存（失焦/回车提交）；只读信息：尺寸、大小、mtime、全部路径
6. 导入：拖拽文件/文件夹到网格 → `item/add`（folder 路径取当前文件夹；文件夹由前端递归展开为文件逐个导入）
7. 右键菜单：添加标签 / 添加到分类 / 移动到文件夹 / 在 Finder 显示 / 评分 0–5 / 回收（回收站视图为恢复、清空）
8. 回收站：查看、单项或批量恢复、清空（二次确认）
9. 实时性：另一进程改动库目录（或第二窗口操作）经 SSE 反映到界面
10. 快捷键：`Delete` 回收/恢复、`Esc` 关浮层、`Cmd/Ctrl+A` 全选
11. 托盘运行：关闭窗口最小化到系统托盘（hawk-server 驻留后台，扩展采集不间断）；托盘左键/菜单唤出主窗口；托盘「退出」才真正退出；驻留期间再次启动应用唤起已有实例

## 非目标（v1 明确不做）

- 瀑布流不等高布局、框选、颜色标签、标签云
- URL/插件导入的界面入口（API 已支持）
- 多素材库并存、服务器版
- 前端单元测试框架（Vitest 暂缓；契约层由 server 的 smoke.sh 兜底）

## 打包与分发

- `electron-builder.yml`：`extraResources` 按平台携带 hawk-server 自包含单文件（`dotnet publish -r win-x64 / osx-arm64 / linux-x64` 产物）
- 前端 `vite build` 产物进 `app.asar`；file:// 加载
- 产物：macOS `hawk.app` 目录（CI 交叉打包 arm64 + x64 后 zip 发布；不做 dmg）/ Windows `hawk.zip`（解压即用）/ Linux AppImage
- CI（后续）：server 的 OpenAPI schema 与前端生成类型的一致性校验，防止契约漂移

## 目录结构

```text
hawk-app/
├── package.json            # 全部依赖与脚本（单包，不做 workspaces）
├── electron-builder.yml
├── electron/
│   ├── main.cjs            # 窗口管理（macOS 原生红绿灯 / Windows/Linux 无边框 + 窗口控制 IPC）、关窗隐藏到托盘 + 系统托盘、单实例锁、拉起/回收 server、token、库选择、白名单 IPC
│   ├── preload.cjs         # contextBridge 白名单通道（换库/文件管理器/剪贴板/拖拽路径/窗口控制/server 进度·就绪·错误事件/退出应用）+ webUtils
├── scripts/
│   ├── gen-types.mjs       # 拉起 server 拉取 OpenAPI schema 生成 TS 类型
│   ├── dev.mjs             # 一键开发：vite + electron（wait-on 5173）
│   ├── build-server.mjs    # dotnet publish 产出指定 RID 的 hawk-server 自包含单文件
│   ├── pack.mjs            # electron-builder 打包（Windows zip / macOS .app / Linux AppImage）
│   ├── test-mobile-web.mjs # 移动端网页冒烟测试编排（临时库 + server + 断言）
│   └── mobile-web-probe.cjs# 测试探针：无 preload 的 sandbox Electron 窗口模拟手机浏览器，输出 JSONL 探针与截图
└── web/                    # Vue 3 + Vite 前端，src/ 详档见「前端信息架构 · 目录结构」
```

## 开发工作流

```bash
npm install
npm run gen:types   # 生成/更新 API 类型（需 hawk-server 已 dotnet build）
npm run dev         # vite(5173) + electron；server 由 electron 拉起（dotnet dll）
npm run build       # vue-tsc --noEmit && vite build
npm run test:mobile # 移动端网页冒烟测试（见下）
```

**移动端网页冒烟测试**（`scripts/test-mobile-web.mjs` + `scripts/mobile-web-probe.cjs`，`npm run test:mobile`）：覆盖桌面 IPC 路径之外的另一半——局域网手机浏览器链路（无 `hawkShell` 的轮询启动）。免第三方依赖：程序化生成含全景/竖图的临时素材库（纯色 PNG，免依赖手写编码器）→ 拉起 hawk-server 托管 web/dist → 以 **无 preload 的 sandbox Electron 窗口（390×844）** 模拟手机浏览器 → JSONL 探针断言：启动屏渲染 → **轮询就绪进入主界面（卡在启动屏即失败——2026-08 轮询未触发的回归即由此检出）** → 网格卡片渲染且无横向溢出（顺带校验齐行布局行宽硬顶）→ 点按卡片开预览且中央图在视口内。产物在 `.tmp/mobile-smoke/`（已 gitignore），成功即清理，失败保留截图供排查。

关键依赖：`vue@3`、`pinia`、`@vueuse/core`、`vite`、`@vitejs/plugin-vue`、`vue-tsc`、`electron`、`electron-builder`、`openapi-typescript`。

纯前端调试（不启 Electron）：`VITE_HAWK_API=http://127.0.0.1:27371 VITE_HAWK_TOKEN=<token> npm run dev:web`，hash 无参数时回退读这两个环境变量。

自检手段：`HAWK_SCREENSHOT=<路径>` 环境变量启动时，主进程在页面加载完成后截图落盘，供无头验证渲染结果。

## 已知缺口与风险

1. **超大库的骨架传输**：`item/skeleton` 一次性回全量 dim，十万级条目约数 MB 本地传输、布局计算 O(n)（毫秒级）；若未来成为瓶颈，骨架可再压缩（数组元组/增量推送）
2. **格式兜底**：后端暂不支持的格式（RAW/HEIC）无缩略图，前端渲染占位图（ext 角标）
3. **token 暴露面**：localhost + hash 注入 + 内存保存，本机风险可控；不写盘
4. **macOS 公证/Windows 签名**：打包分发阶段再处理，v1 不涉及

## 里程碑

| 阶段 | 产出 |
| ---- | ---- |
| M1 骨架 | 仓库结构、dev 一键起、server 拉起与 token 注入、app/info 打通 |
| M2 浏览 | 侧栏树 + 网格 + 分页 + 缩略图 |
| M3 整理 | 检查器编辑、文件夹增删改、右键菜单、回收/恢复 |
| M4 导入与实时 | 拖拽导入、SSE 刷新、回收站清空 |
| M5 打包 | electron-builder 出三平台包 |

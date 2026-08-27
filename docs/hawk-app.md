# hawk-app 设计（Electron 壳 + Vue 前端）

桌面版应用的设计文档。约束来源：[architecture.md](architecture.md)（sidecar 进程模型）、[tech-stack.md](tech-stack.md)（Vue 3 + TypeScript + Vite）、[server-rest-api-v1.md](server-rest-api-v1.md)（接口契约）。

## 目标

交付单机可用的素材管理桌面应用 v1，界面布局参考 Eagle，覆盖核心工作流：浏览、搜索、整理文件夹、标记素材（标签/评分/备注）、导入、回收站。

## Eagle 界面参考与取舍

Eagle 主窗口的关键特征：

```text
┌────────────────────────────────────────────────────────────────┐
│ 标题栏：侧栏开关 · 前进/后退 · 面包屑 ‖ 缩略图滑杆 ‖ 筛选 · 搜索 · 窗口控制 │
├──────────┬─────────────────────────────────────┬───────────────┤
│ 侧栏      │ 素材网格（瀑布流）                    │ 检查器         │
│ · 素材库  │  · 缩略图卡片                        │ · 大图预览     │
│ · 文件夹树│  · 多选 / 框选                       │ · 名称/评分    │
│ · 标签    │  · 右键菜单                          │ · 标签/备注    │
│ · 回收站  │  · 双击放大预览                      │ · URL/文件信息 │
└──────────┴─────────────────────────────────────┴───────────────┘
```

无边框窗口 + 自绘通栏标题栏（含窗口控制）、深色主题、缩略图优先的网格浏览、右侧检查器即选即改，是 Eagle 体验的核心，全部采纳。

取舍：

| Eagle 特性 | 决策 | 说明 |
| ---------- | ---- | ---- |
| 三栏布局 + 通栏自绘标题栏 | 采纳 | 标题栏集成侧栏开关/前进后退/面包屑/缩略图滑杆/筛选/搜索/窗口控制；macOS 用系统原生红绿灯（`titleBarStyle: 'hidden'`），Windows/Linux 无边框（`frame: false`）+ 自绘窗口控制；不做状态栏 |
| 深色主题 | 采纳 | 自定义 CSS，不引组件库 |
| 瀑布流（不等高）网格 | 采纳（齐行布局） | Eagle 实为「行内等高、宽度按宽高比」的 justified 布局；自研贪心装行算法，不引库 |
| 侧栏标签云/智能文件夹 | 采纳（标签列表） | Category 维度落地后侧栏含分类树与标签列表（见 category.md）；智能文件夹不做 |
| 框选 | 不做 | 保留 Shift/Cmd 点选 |
| 评分筛选、颜色标签 | 评分筛选做（标题栏），颜色标签不做 | API 支持 star 过滤 |
| 浏览器扩展采集 | 不做 | 属生态接入，后端 API 已就绪 |

## 进程模型与启动流程

严格遵循 architecture.md 的 sidecar 模式：前端不依赖 Electron IPC 做业务通信，主进程只管窗口、拉起/回收后端、注入 token。

```text
Electron 主进程启动
  → 读用户配置（userData/hawk-app.json）取最近素材库；无 → 弹出目录选择框
  → 生成随机 token
  → spawn hawk-server（开发：dotnet 运行 dll；打包：process.resourcesPath 内二进制）
      环境变量传入 HAWK_TOKEN，参数 --library <path> --port 27371
  → 解析子进程 stdout 的 `HAWK_READY <address> token=...` 行取得实际端口
    （兜底：轮询默认端口 /health）
  → 创建 BrowserWindow，加载：
      开发：http://localhost:5173/#api=<addr>&token=<token>
      打包：file://.../index.html#api=<addr>&token=<token>
退出
  → 杀掉子进程（含异常退出路径，防止孤儿进程）
```

token 经 URL hash 注入渲染进程（hash 不进 HTTP 请求、不进 History API），前端读取后保存在内存，不写 localStorage。

**preload 白名单**（contextBridge，只暴露这些，与业务数据无关）：

| 通道 | 用途 |
| ---- | ---- |
| `selectLibrary()` | 更换素材库：弹目录选择框 → 主进程杀掉旧 server 并用新库重启 → 重载窗口 |
| `showInFinder(path)` | 右键「在 Finder 中显示」，主进程 `shell.showItemInFolder` |
| `getPathForFile(file)` | 拖拽导入时取文件绝对路径（Electron `webUtils`），供 `item/add` 使用 |
| `minimizeWindow()` / `toggleMaximizeWindow()` / `closeWindow()` | 自绘标题栏的窗口控制（仅 Windows/Linux；macOS 用系统原生红绿灯）；toggle 返回切换后的最大化状态 |
| `onWindowMaximized(cb)` | 订阅最大化状态变化（含 Aero Snap 等系统途径），标题栏据此切换 最大化/还原 图标；返回退订函数 |

## 契约与类型生成

- 前端 TS 类型从 OpenAPI schema 生成（openapi-typescript），**不手写对接口**
- `npm run gen:types`：脚本启动 hawk-server（临时目录建库）→ 拉 `/openapi/v1.json` → 生成 `web/src/api/schema.d.ts` → 杀掉 server。生成文件入库，schema 变更时重新生成
- API client 统一处理：Bearer 头、信封解包（`status`/`data`/`error`）、错误码异常、SSE 用 `?token=`（EventSource 无法设请求头）

## 前端信息架构

### 布局

```text
┌─────────────────────────────────────────────────────────────────────┐
│ TitleBar  [侧栏][‹][›] 面包屑·已选N  [−滑杆＋]  [star▾][排序▾][🔍搜索][—][▢][✕] │
├────────────┬──────────────────────────────────┬─────────────────────┤
│ Sidebar    │ ItemGrid                         │ Inspector           │
│  库名 ⌄    │  齐行网格：行内等高，宽度按宽高比     │  预览图(格式角标)    │
│  全部素材 N │  卡片下方：name.ext + 尺寸         │  名称(可改)          │
│  ───────── │  无限滚动分页（100/页）            │  注释(可改)          │
│  文件夹树   │  懒加载 <img loading=lazy>        │  URL(可改)          │
│  (增/删/改) │  多选：Shift 连选 / Cmd 点选       │  标签 chips         │
│  分类树     │  双击/空格 → 预览浮层（Esc 关闭）   │  分类 chips ＋      │
│  (增/删/改) │  右键：标签/分类/文件夹/回收        │  文件夹 chips ＋    │
│  标签列表 N │  拖入文件 → 导入                   │  基本信息(评分并入)  │
│  (增/删/改) │                                  │  文件位置            │
│  回收站 N   │                                  │                     │
└────────────┴──────────────────────────────────┴─────────────────────┘
```

窗口标题栏按平台区分：macOS 隐藏系统标题栏但保留原生红绿灯（`titleBarStyle: 'hidden'`，`trafficLightPosition` 按 40px 栏高垂直居中，悬停 glyph/失焦置灰/全屏行为由系统保证），标题栏内容左移 78px 避让；Windows/Linux 为无边框窗口（`frame: false`），右端自绘窗口控制按钮经 preload 白名单 IPC 驱动主进程。标题栏（`TitleBar.vue`）为通栏自绘，整条是窗口拖拽区（双击空白切换最大化），交互控件单独 `no-drag`。无独立状态栏：计数在侧栏各行徽章（全部素材/文件夹/分类/标签/回收站），选中数在标题栏面包屑旁。侧栏行首为描边小图标（Icon.vue，feather 风格 inline SVG）。

网格为**齐行布局**（justified layout，与 Eagle 一致）：贪心装行，非末行按容器宽精确反推行高，单元格与图片同宽高比——图片完整显示不裁切。由 ItemGrid 按宽高比计算 flex 行（ResizeObserver 驱动），非 CSS grid。多选面板（Inspector）提供批量添加标签/分类/移动文件夹、批量评分、总大小与堆叠预览。

当前位置体现在侧栏选中态与标题栏：文件夹/分类视图渲染可点击面包屑（根 = 全部素材，逐级跳转），其余视图为固定标题；标题栏前进/后退在会话内浏览历史（`setView` 压栈，重命名跟随/删除回退就地修正当前条目）中移动。回收站视图下右键菜单变为「恢复 / 彻底删除（清空回收站，二次确认）」。文件夹与分类树节点的计数由后端 `folder/list`、`category/list` 的 `count` 字段提供（含子级、不含回收站、按 item 去重）。

### 目录结构（web/src）

```text
web/
├── index.html
├── tsconfig.json
└── src/
    ├── main.ts                # 入口：解析 hash 注入 api/token、创建 Pinia、挂载 App
    ├── App.vue                # 布局骨架；挂载全局 composables（快捷键/拖拽导入）与浮层
    ├── types.ts               # 业务类型（ViewState/QueryState/MenuItem）；Item 等从 schema.d.ts 别名导出
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
    ├── components/
    │   ├── TitleBar.vue
    │   ├── WindowControls.vue
    │   ├── Sidebar.vue
    │   ├── FolderTreeNode.vue
    │   ├── ItemGrid.vue
    │   ├── ItemCard.vue
    │   ├── Inspector.vue
    │   ├── TagEditor.vue      # 标签 chip 编辑器（Inspector 的子组件）
    │   ├── StarRating.vue     # 点星评分（Inspector/右键菜单共用）
    │   ├── PromptDialog.vue   # 文本输入模态（添加标签/新建文件夹）
    │   ├── FolderPickerDialog.vue # 文件夹选择模态（移动到文件夹）
    │   ├── PreviewOverlay.vue
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
export type ViewState = { kind: 'all' } | { kind: 'folder'; path: string } | { kind: 'trash' };
export interface QueryState {
  keywords: string[];
  star?: number;
  orderBy: 'modification_time' | 'name' | 'size' | 'star';
  order: 'asc' | 'desc';
}
export interface MenuItem { label: string; danger?: boolean; separator?: boolean; action?: () => void }
```

### API 层

**client.ts**——模块级单例，启动时从 `location.hash` 解析：

```ts
export class ApiError extends Error {
  constructor(public code: string, message: string, public httpStatus: number) { super(message); }
}
export function initApiFromLocation(): { api: string; token: string } | null
// 优先解析 location.hash（#api=...&token=...，Electron 注入）；
// 无 hash 时回退 import.meta.env 的 VITE_HAWK_API / VITE_HAWK_TOKEN（纯前端调试）；都缺 → null（启动失败态）
export async function request<T>(method: string, path: string,
  opts?: { body?: unknown; query?: Record<string, string> }): Promise<T>
// 行为：拼 Bearer 头；信封解包（status==='error' → throw ApiError）；网络错误 → ApiError('NETWORK')；无 data → undefined
```

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
  itemDetail(id: string): Promise<Item>;
  itemCount(): Promise<number>;
  itemAddByPath(path: string, opts?: { name?: string; folder_path?: string; tags?: string[] }): Promise<{ item: Item; already_existed: boolean }>;
  itemUpdate(id: string, patch: { name?; tags?; star?; annotation?; url?; folder_path? }, path?: string): Promise<Item>;
  itemDelete(id: string, path?: string): Promise<void>;
  itemRestore(id: string, path?: string): Promise<void>;
  refreshThumbnail(id: string): Promise<void>;
  trashClear(): Promise<void>;
  thumbnailUrl(id: string, size?: 256 | 1024): string;  // 拼 ?token= 的 <img> URL
  fileUrl(id: string): string;  // 原图 URL（预览浮层用），同样拼 ?token=
};
```

**缩略图与原图的鉴权**：`<img>` 无法带请求头，采用 `?token=` 查询参数。需后端配合：`TokenAuthMiddleware` 对 `GET /api/v1/item/thumbnail`、`GET /api/v1/item/file` 放行查询参数 token（与 events 同款）。缩略图与原图 URL 因此稳定，配合 `Cache-Control: immutable` 获得浏览器级缓存。检查器 1024 大图同理。

**events.ts**：

```ts
export function connectEvents(handlers: {
  onAdded(item: Item): void; onUpdated(item: Item): void;
  onTrashed(id: string): void; onRestored(item: Item): void; onRemoved(id: string): void;
  onReconnect(): void;   // EventSource 断线重连成功后全量对齐
}): () => void;           // 返回断开函数（App 卸载/换库时调）
```

### Pinia store（stores/library.ts）

单一 store `useLibraryStore`，组件不直接调 api（除缩略图 URL 拼接），一切经 action：

```ts
// ---- state ----
view: ViewState;                 // 默认 all
query: QueryState;               // 默认 { keywords: [], orderBy: 'modification_time', order: 'desc' }
items: Item[];                   // 当前视图已加载页（无限滚动累加）
total: number; loading: boolean; endReached: boolean;
selection: string[];             // 选中 id，有序；末位为主选中/连选锚点
folders: FolderNode | null;      // 完整树（含根）
library: LibraryInfo | null;
thumbSize: number;               // 网格卡片边长偏好（默认 160，内存态不持久化）
sidebarVisible: boolean;         // 侧栏显隐（标题栏开关，默认开）
previewId: string | null;        // 预览浮层
toast: string | null;            // 轻提示（3s 自动清除）
// 会话内浏览历史：viewHistory/historyIndex，setView 压栈，数据变更修正就地替换当前条目

// ---- getters ----
isTrash: boolean;                // view.kind === 'trash'
currentFolderPath: string | null;
selectedItems: Item[];
primarySelected: Item | null;    // selection 末位对应的 item
previewItem / previewPrevId / previewNextId: 浮层与左右切换

// ---- actions ----
init(): Promise<void>;           // libraryInfo + folders + resetList；失败进启动失败态
setView(v: ViewState): void;                     // 切视图：压浏览历史 → 清空选择 → resetList
goBack() / goForward(): void;                    // 标题栏前进/后退（canGoBack/canGoForward 驱动禁用态）
toggleSidebar(): void;                           // 侧栏显隐开关
setQuery(patch: Partial<QueryState>): void;        // → resetList
resetList(): Promise<void>;      // items 清空 → fetchMore 第一页
fetchMore(): Promise<void>;      // offset=items.length, limit=100；in_trash/folders 由 view 派生
refresh(): Promise<void>;        // 重查已加载范围并替换（SSE 用，保滚动位置）
select(id: string, mod?: 'range' | 'toggle'): void;
selectAll(): void; clearSelection(): void;
updateItem(id: string, patch): Promise<void>;      // 就地更新 items；ApiError → toast
trashSelected(): Promise<void>; restoreSelected(): Promise<void>;
clearTrash(): Promise<void>;                       // 调用方先二次确认
importPaths(paths: string[]): Promise<void>;       // 逐个 itemAddByPath；汇总 toast（成功 n，已存在 m）
refreshFolders(): Promise<void>;
openPreview(id): void; closePreview(): void; navigatePreview(step: 1 | -1): void;
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
| `App.vue` | — | — | 布局骨架（标题栏通栏 + 三栏；`no-sidebar` 时侧栏列归零）；启动流程 boot()：initApiFromLocation（失败显示「请从 hawk 桌面端启动」）→ store.init → connectEvents；`onMounted` 跑 boot() 并监听 `hashchange`——引导页选库后主进程仅改 URL hash 注入连接参数（same-document 导航，页面不重载），需重新 boot() 才能切到主界面；挂载全局快捷键/拖拽 composable；挂载 PreviewOverlay/ContextMenu/toast；引导页/失败页带拖拽条与窗口控制 |
| `TitleBar.vue` | — | — | Eagle 式通栏标题栏（无边框窗口拖拽区，双击空白切换最大化）：侧栏开关、前进/后退、位置面包屑（文件夹/分类逐级跳转）+ 选中计数、缩略图滑杆（−/＋步进）、读写 store.query（搜索框回车按空格拆 keywords、star 筛选下拉、颜色筛选 chip、排序下拉）、窗口控制 |
| `WindowControls.vue` | — | — | 最小化/最大化(还原)/关闭按钮（Windows/Linux 风格，固定右上）；macOS 不渲染（系统原生红绿灯）；控件区由本组件自带 `app-region: no-drag`（父组件 TitleBar 的 scoped no-drag 规则命中不到子组件按钮，缺了会被拖拽区拦截真实点击）；仅 Electron 内渲染；最大化态经 `onWindowMaximized` 订阅同步 |
| `Sidebar.vue` | — | — | 「全部素材」、FolderTreeNode 递归、「回收站」；选中态反映 store.view |
| `FolderTreeNode.vue` | `node: FolderNode`、`depth: number` | — | 内部态：expanded、editing（重命名/新建的内联 input）；点击 setView；右键菜单：新建子文件夹/重命名/删除（确认） |
| `ItemGrid.vue` | — | — | 滚动容器渲染 store.items；sentinel 翻页；空态 EmptyState；右键/双击/点选转发 store |
| `ItemCard.vue` | `item: Item`、`selected: boolean`、`size: number` | `select(id, MouseEvent)`、`open(id)`、`menu(id, x, y)` | 缩略图（`loading=lazy`，加载失败显示 ext 占位块）、名称、★ 角标 |
| `Inspector.vue` | — | — | 单选：1024 预览 + 调色板色块行（点击在当前视图范围内按颜色检索，再点当前色清除）+ 可编辑字段（失焦提交 updateItem；名称/注释为自动增高 textarea，名称回车提交且换行转空格，注释支持多行、Ctrl+Enter 提交）；多选：数量 + 批量按钮；只读信息区（ext/尺寸/大小/mtime/id 短码/全部路径） |
| `TagEditor.vue` | `modelValue: string[]` | `update:modelValue` | chip + 删除；「＋」按钮展开内联输入（带既有标签候选 datalist），Enter/失焦提交、Esc 取消（trim 去重） |
| `StarRating.vue` | `modelValue: number` | `update:modelValue` | 5 星；点当前星值 → 清零 |
| `PromptDialog.vue` | `title, placeholder?` | `confirm(value)`、`cancel` | 通用文本输入模态（Enter 提交/Esc 取消） |
| `FolderPickerDialog.vue` | `title` | `confirm(path)`、`cancel` | 文件夹选择模态（扁平树下拉） |
| `PreviewOverlay.vue` | `item: Item` | `close`、`navigate(1\|-1)` | 全屏展示原图（`/item/file`）；滚轮以光标为中心缩放、拖拽平移、双击复位；Esc/点遮罩/空格关闭；←/→ 切换 |
| `ContextMenu.vue` | — | — | 读 useContextMenu 状态渲染；点外部/Esc 关闭 |
| `EmptyState.vue` | `text: string` | — | 空态文案与「拖入文件开始」提示 |

### composables

通用能力不重复造：无限滚动用 VueUse `useIntersectionObserver` 直接写在 ItemGrid.vue（观察底部哨兵，`!loading && !endReached` 时翻页）；拖拽用 `useDropZone`；全局监听用 `useEventListener`。业务 composable 只保留三个：

| composable | 签名与行为 |
| ---------- | ---------- |
| `useContextMenu()` | 模块级单例响应式状态 `{visible, x, y, items}`（全局唯一菜单）；`open(items, MouseEvent)` 定位（防出屏翻转）；`close()` |
| `useDragImport()` | `useDropZone` 接 drop → `webkitGetAsEntry()` 递归展开文件夹 → `webUtils.getPathForFile` 取绝对路径 → store.importPaths |
| `useShortcuts()` | 全局 keydown：焦点在 input/textarea 时跳过；`Delete/Backspace` → 按视图 trashSelected/restoreSelected；`Esc` → 关浮层/菜单；`Cmd/Ctrl+A` → selectAll；`←/→`（浮层打开时）→ navigatePreview。另有 main.ts 的捕获阶段拦截：IME 组合态（中文输入法选词）中的 Enter/Escape 不下发——Enter 是确认候选而非提交，Esc 是关候选窗而非取消 |

### 样式约定

深色主题，CSS 变量集中 `styles.css :root`：

```css
--bg-0: #1e1e1e;  /* 主区 */  --bg-1: #252526;  /* 侧栏 */  --bg-2: #2d2d30;  /* 检查器/卡片 */
--fg-0: #e8e8e8;  --fg-1: #9d9d9d;  --accent: #4f8cff;  --danger: #e5534b;  --border: #3c3c3c;
```

布局用 CSS Grid（标题栏通栏 40px + 内容区 `220px 1fr 280px`，侧栏可经标题栏开关隐藏归零）；网格卡片 `repeat(auto-fill, minmax(var(--thumb-size), 1fr))`，卡片内缩略图定高 + `object-fit: contain`。

### 错误处理

ApiError 统一在 store action 捕获 → `showToast`（错误码 → 中文文案映射：`FILE_EXISTS`→「同名文件已存在」、`ITEM_NOT_FOUND`→「素材不存在或已被移除」……其余透传 message）。toast 固定底部居中，3s 自动消失。启动级失败（无 token / 连不上 server）渲染整页错误态而非 toast。

### SSE 增量刷新策略

订阅 `/api/v1/events`，按事件类型处理：

| 事件 | 处理 |
| ---- | ---- |
| `item.updated` | 负载是完整 Item。无过滤的「全部素材」视图**就地替换**（成员资格不可能变化）；过滤视图/激活查询条件时防抖 200ms 重查当前页（成员判定以服务端查询为准，如摘掉当前分类后 item 即时消失）。updateItem 响应走同一入口 |
| `item.added` / `item.restored` | 涉及排序位置，防抖 200ms 重查当前页 |
| `item.trashed` / `item.removed` | 普通视图就地移除；回收站视图防抖重查 |
| 任何事件 | 防抖刷新文件夹树（见「已知缺口」） |

断线自动重连（EventSource 原生行为），重连后全量重查对齐。

## 功能清单 v1（验收标准）

1. 启动选库：首次启动弹目录选择；记住上次素材库与上次浏览的文件夹视图（按库路径存 localStorage，文件夹已删则回退全部素材）；菜单可更换库
2. 侧栏：全部素材 / 文件夹树 / 分类树 / 标签列表（三个分区均支持「＋」新建与右键重命名/删除）/ 回收站
3. 网格：缩略图懒加载、无限滚动、单选/Shift 连选/Cmd 点选、双击预览浮层
4. 搜索与筛选：关键词（命中名称/备注）、star 精确筛选、四种排序双向
5. 检查器：1024 预览；名称、标签（chip 增删）、评分（点星）、备注、URL 编辑即存（失焦/回车提交）；只读信息：尺寸、大小、mtime、全部路径
6. 导入：拖拽文件/文件夹到网格 → `item/add`（folder 路径取当前文件夹；文件夹由前端递归展开为文件逐个导入）
7. 右键菜单：添加标签 / 添加到分类 / 移动到文件夹 / 在 Finder 显示 / 评分 0–5 / 回收（回收站视图为恢复、清空）
8. 回收站：查看、单项或批量恢复、清空（二次确认）
9. 实时性：另一进程改动库目录（或第二窗口操作）经 SSE 反映到界面
10. 快捷键：`Delete` 回收/恢复、`Esc` 关浮层、`Cmd/Ctrl+A` 全选

## 非目标（v1 明确不做）

- 瀑布流不等高布局、框选、颜色标签、标签云
- 虚拟滚动（十万级素材再上 vue-virtual-scroller；无限滚动 + 懒加载先行）
- URL/插件导入的界面入口（API 已支持）
- 多素材库并存、服务器版
- 托盘
- 前端单元测试框架（Vitest 暂缓；契约层由 server 的 smoke.sh 兜底）

## 打包与分发

- `electron-builder.yml`：`extraResources` 按平台携带 hawk-server 自包含单文件（`dotnet publish -r win-x64 / osx-arm64 / linux-x64` 产物）
- 前端 `vite build` 产物进 `app.asar`；file:// 加载
- 产物：macOS `hawk.app` 目录（CI 交叉打包 arm64 + x64 后 zip 发布；不做 dmg）/ Windows portable / Linux AppImage
- CI（后续）：server 的 OpenAPI schema 与前端生成类型的一致性校验，防止契约漂移

## 目录结构

```text
hawk-app/
├── package.json            # 全部依赖与脚本（单包，不做 workspaces）
├── electron-builder.yml
├── electron/
│   ├── main.cjs            # 窗口管理（macOS 原生红绿灯 / Windows/Linux 无边框 + 窗口控制 IPC）、拉起/回收 server、token、库选择、白名单 IPC
│   └── preload.cjs         # contextBridge 白名单通道（换库/文件管理器/拖拽路径/窗口控制）+ webUtils
├── scripts/
│   ├── gen-types.mjs       # 拉起 server 拉取 OpenAPI schema 生成 TS 类型
│   └── dev.mjs             # 一键开发：vite + electron（wait-on 5173）
└── web/                    # Vue 3 + Vite 前端，src/ 详档见「前端信息架构 · 目录结构」
```

## 开发工作流

```bash
npm install
npm run gen:types   # 生成/更新 API 类型（需 hawk-server 已 dotnet build）
npm run dev         # vite(5173) + electron；server 由 electron 拉起（dotnet dll）
npm run build       # vue-tsc --noEmit && vite build
```

关键依赖：`vue@3`、`pinia`、`@vueuse/core`、`vite`、`@vitejs/plugin-vue`、`vue-tsc`、`electron`、`electron-builder`、`openapi-typescript`。

纯前端调试（不启 Electron）：`VITE_HAWK_API=http://127.0.0.1:27371 VITE_HAWK_TOKEN=<token> npm run dev:web`，hash 无参数时回退读这两个环境变量。

自检手段：`HAWK_SCREENSHOT=<路径>` 环境变量启动时，主进程在页面加载完成后截图落盘，供无头验证渲染结果。

## 已知缺口与风险

1. **文件夹树无 SSE 事件**：外部进程增删文件夹时前端不自动刷新。v1 缓解：item 事件时防抖重拉文件夹树；后续建议后端补 `folder.changed` 事件（开放问题，实现前定）
2. **大图库网格性能**：无限滚动不卸载已渲染节点，数千项后 DOM 变大；`loading=lazy` 控制解码开销。超十万级需虚拟滚动，届时评估 vue-virtual-scroller
3. **格式兜底**：后端暂不支持的格式（RAW/HEIC）无缩略图，前端渲染占位图（ext 角标）
4. **token 暴露面**：localhost + hash 注入 + 内存保存，本机风险可控；不写盘
5. **后端配合点**：`TokenAuthMiddleware` 需对 `GET /api/v1/item/thumbnail` 放行 `?token=`（`<img>` 无法带请求头，与 events 同款处理）——实现期修改，仅此一处
6. **macOS 公证/Windows 签名**：打包分发阶段再处理，v1 不涉及

## 里程碑

| 阶段 | 产出 |
| ---- | ---- |
| M1 骨架 | 仓库结构、dev 一键起、server 拉起与 token 注入、app/info 打通 |
| M2 浏览 | 侧栏树 + 网格 + 分页 + 缩略图 |
| M3 整理 | 检查器编辑、文件夹增删改、右键菜单、回收/恢复 |
| M4 导入与实时 | 拖拽导入、SSE 刷新、回收站清空 |
| M5 打包 | electron-builder 出三平台包 |

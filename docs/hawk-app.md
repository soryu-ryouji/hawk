# hawk-app 设计（Electron 壳 + React 前端）

桌面版应用的设计文档。约束来源：[architecture.md](architecture.md)（sidecar 进程模型）、[tech-stack.md](tech-stack.md)（React + TypeScript + Vite）、[server-rest-api-v1.md](server-rest-api-v1.md)（接口契约）。

## 目标

交付单机可用的素材管理桌面应用 v1，界面布局参考 Eagle，覆盖核心工作流：浏览、搜索、整理文件夹、标记素材（标签/评分/备注）、导入、回收站。

## Eagle 界面参考与取舍

Eagle 主窗口的关键特征：

```text
┌────────────────────────────────────────────────────────────────┐
│ 工具栏：搜索框 · 筛选 · 排序 · 缩略图尺寸                         │
├──────────┬─────────────────────────────────────┬───────────────┤
│ 侧栏      │ 素材网格（瀑布流）                    │ 检查器         │
│ · 素材库  │  · 缩略图卡片                        │ · 大图预览     │
│ · 文件夹树│  · 多选 / 框选                       │ · 名称/评分    │
│ · 标签    │  · 右键菜单                          │ · 标签/备注    │
│ · 回收站  │  · 双击放大预览                      │ · URL/文件信息 │
├──────────┴─────────────────────────────────────┴───────────────┤
│ 状态栏：数量统计                                                 │
└────────────────────────────────────────────────────────────────┘
```

深色主题、缩略图优先的网格浏览、右侧检查器即选即改，是 Eagle 体验的核心，v1 全部采纳。

v1 取舍：

| Eagle 特性 | v1 决策 | 说明 |
| ---------- | ------- | ---- |
| 三栏布局 + 工具栏 + 状态栏 | 采纳 | |
| 深色主题 | 采纳 | 自定义 CSS，不引组件库 |
| 瀑布流（不等高）网格 | **不采纳** | v1 用固定高宽比网格（`object-fit: contain` 居中），实现简单且缩略图本身已等比缩放 |
| 侧栏标签云/智能文件夹 | **不做** | 后端无「全部标签」枚举 API，v1 侧栏只有文件夹树与回收站 |
| 框选 | 不做 | 保留 Shift/Cmd 点选 |
| 评分筛选、颜色标签 | 评分筛选做（工具栏），颜色标签不做 | API 支持 star 过滤 |
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

**preload 白名单**（contextBridge，只暴露这三个，与业务数据无关）：

| 通道 | 用途 |
| ---- | ---- |
| `selectLibrary()` | 更换素材库：弹目录选择框 → 主进程杀掉旧 server 并用新库重启 → 重载窗口 |
| `showInFinder(path)` | 右键「在 Finder 中显示」，主进程 `shell.showItemInFolder` |
| `getPathForFile(file)` | 拖拽导入时取文件绝对路径（Electron `webUtils`），供 `item/add` 使用 |

## 契约与类型生成

- 前端 TS 类型从 OpenAPI schema 生成（openapi-typescript），**不手写对接口**
- `npm run gen:types`：脚本启动 hawk-server（临时目录建库）→ 拉 `/openapi/v1.json` → 生成 `web/src/api/schema.d.ts` → 杀掉 server。生成文件入库，schema 变更时重新生成
- API client 统一处理：Bearer 头、信封解包（`status`/`data`/`error`）、错误码异常、SSE 用 `?token=`（EventSource 无法设请求头）

## 前端信息架构

### 布局

```text
┌────────────────────────────────────────────────────────────────┐
│ Toolbar  [搜索 keywords] [star筛选▾] [排序▾] [缩略图尺寸滑杆]     │
├────────────┬──────────────────────────────────┬────────────────┤
│ Sidebar    │ ItemGrid                         │ Inspector      │
│  全部素材   │  等比网格卡片（缩略图+名称+评分）    │  1024 预览图    │
│  ───────── │  无限滚动分页（50/页）             │  名称(可改)     │
│  文件夹树   │  懒加载 <img loading=lazy>        │  ★★★★★        │
│  (增/删/改) │  多选：Shift 连选 / Cmd 点选       │  标签编辑器     │
│  ───────── │  双击 → 预览浮层（Esc 关闭）        │  备注(可改)     │
│  回收站     │  右键：回收/恢复/评分/Finder 显示   │  URL(可改)     │
│            │  拖入文件 → 导入                   │  文件信息/路径  │
├────────────┴──────────────────────────────────┴────────────────┤
│ 状态栏：共 N 项 · 已选 M 项                                       │
└────────────────────────────────────────────────────────────────┘
```

当前位置（全部/文件夹路径/回收站）体现在侧栏选中态；回收站视图下右键菜单变为「恢复 / 彻底删除（清空回收站，二次确认）」。

### 组件划分

```text
App
├── TitleBar?（不自定义标题栏，用系统原生）
├── Toolbar          搜索输入、star 筛选、排序、缩略图尺寸
├── Sidebar
│   └── FolderTreeNode（递归，右键：新建/重命名/删除）
├── ItemGrid
│   └── ItemCard × N（缩略图、名称、评分角标、选中态）
├── Inspector        单选编辑；多选显示数量与批量操作（回收/评分）
├── PreviewOverlay   全屏预览
└── ContextMenu      自绘（Electron 原生菜单在 Windows/macOS 表现不一，自绘统一）
```

### 状态管理

单个 zustand store，服务端状态不进 store 持久层，列表数据按查询缓存于内存：

```ts
{
  view: { kind: 'all' } | { kind: 'folder', path } | { kind: 'trash' },
  query: { keywords: string[], star?: number, orderBy, order },
  items: Item[], total: number, loading: boolean,
  selection: Set<string>,          // 选中 item id
  folders: FolderNode[],           // 文件夹树
  library: LibraryInfo,
  // actions：fetchItems(重置/翻页)、select*、updateItem、trash/restore、clearTrash、importFiles、refreshFolders
}
```

### SSE 增量刷新策略

订阅 `/api/v1/events`，按事件类型处理：

| 事件 | 处理 |
| ---- | ---- |
| `item.updated` | 负载是完整 Item，列表内**就地替换** |
| `item.added` / `item.restored` | 涉及排序位置，防抖 200ms 重查当前页 |
| `item.trashed` / `item.removed` | 普通视图就地移除；回收站视图防抖重查 |
| 任何事件 | 防抖刷新文件夹树（见「已知缺口」） |

断线自动重连（EventSource 原生行为），重连后全量重查对齐。

## 功能清单 v1（验收标准）

1. 启动选库：首次启动弹目录选择；记住上次库；菜单可更换库
2. 侧栏：全部素材 / 文件夹树（展开收起、右键新建/重命名/删除，删除进回收站）/ 回收站
3. 网格：缩略图懒加载、无限滚动、单选/Shift 连选/Cmd 点选、双击预览浮层
4. 搜索与筛选：关键词（命中名称/备注）、star 精确筛选、四种排序双向
5. 检查器：1024 预览；名称、标签（chip 增删）、评分（点星）、备注、URL 编辑即存（失焦/回车提交）；只读信息：尺寸、大小、mtime、全部路径
6. 导入：拖拽文件/文件夹到网格 → `item/add`（folder 路径取当前文件夹）
7. 右键菜单：回收 / 恢复 / 在 Finder 显示 / 评分 0–5
8. 回收站：查看、单项或批量恢复、清空（二次确认）
9. 实时性：另一进程改动库目录（或第二窗口操作）经 SSE 反映到界面
10. 快捷键：`Delete` 回收/恢复、`Esc` 关浮层、`Cmd/Ctrl+A` 全选

## 非目标（v1 明确不做）

- 瀑布流不等高布局、框选、颜色标签、标签云
- 虚拟滚动（十万级素材再上 react-window；无限滚动 + 懒加载先行）
- URL/插件导入的界面入口（API 已支持）
- 多素材库并存、服务器版
- 自定义标题栏、托盘
- 前端单元测试框架（Vitest 暂缓；契约层由 server 的 smoke.sh 兜底）

## 打包与分发

- `electron-builder.yml`：`extraResources` 按平台携带 hawk-server 自包含单文件（`dotnet publish -r win-x64 / osx-arm64 / linux-x64` 产物）
- 前端 `vite build` 产物进 `app.asar`；file:// 加载
- 产物：macOS dmg / Windows nsis / Linux AppImage
- CI（后续）：server 的 OpenAPI schema 与前端生成类型的一致性校验，防止契约漂移

## 目录结构

```text
hawk-app/
├── package.json            # 全部依赖与脚本（单包，不做 workspaces）
├── electron-builder.yml
├── electron/
│   ├── main.cjs            # 窗口、拉起/回收 server、token、库选择、白名单 IPC
│   └── preload.cjs         # contextBridge（三通道）+ webUtils
├── scripts/
│   ├── gen-types.mjs       # 拉起 server 拉取 OpenAPI schema 生成 TS 类型
│   └── dev.mjs             # 一键开发：vite + electron（wait-on 5173）
└── web/
    ├── index.html
    └── src/
        ├── main.tsx / App.tsx
        ├── api/            # client.ts（信封/错误/token）、schema.d.ts（生成）、events.ts（SSE）
        ├── store.ts        # zustand
        ├── components/     # Toolbar / Sidebar / ItemGrid / ItemCard / Inspector / PreviewOverlay / ContextMenu
        └── styles.css      # 深色主题，CSS 变量
```

## 开发工作流

```bash
npm install
npm run gen:types   # 生成/更新 API 类型（需 hawk-server 已 dotnet build）
npm run dev         # vite(5173) + electron；server 由 electron 拉起（dotnet dll）
```

自检手段：`HAWK_SCREENSHOT=<路径>` 环境变量启动时，主进程在页面加载完成后截图落盘，供无头验证渲染结果。

## 已知缺口与风险

1. **文件夹树无 SSE 事件**：外部进程增删文件夹时前端不自动刷新。v1 缓解：item 事件时防抖重拉文件夹树；后续建议后端补 `folder.changed` 事件（开放问题，实现前定）
2. **大图库网格性能**：无限滚动不卸载已渲染节点，数千项后 DOM 变大；`loading=lazy` 控制解码开销。超十万级需虚拟滚动，届时评估 react-window
3. **格式兜底**：后端暂不支持的格式（RAW/HEIC）无缩略图，前端渲染占位图（ext 角标）
4. **token 暴露面**：localhost + hash 注入 + 内存保存，本机风险可控；不写盘
5. **macOS 公证/Windows 签名**：打包分发阶段再处理，v1 不涉及

## 里程碑

| 阶段 | 产出 |
| ---- | ---- |
| M1 骨架 | 仓库结构、dev 一键起、server 拉起与 token 注入、app/info 打通 |
| M2 浏览 | 侧栏树 + 网格 + 分页 + 缩略图 |
| M3 整理 | 检查器编辑、文件夹增删改、右键菜单、回收/恢复 |
| M4 导入与实时 | 拖拽导入、SSE 刷新、回收站清空 |
| M5 打包 | electron-builder 出三平台包 |

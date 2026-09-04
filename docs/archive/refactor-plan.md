# 前端代码评审问题清单与改造计划

> 本文档是 hawk-app 前端（`hawk-app/web/`、`hawk-app/electron/`）代码评审的完整问题清单，
> 供后续实施者（人或模型）直接执行。每个问题含：现状证据、风险、改造方案、验收标准。
> 相关设计文档：[hawk-app.md](../frontend/hawk-app.md)（实施完成后必须同步更新）。

## 实施状态（全部完成，文档存档）

| 编号 | 状态 | 落点 |
| ---- | ---- | ---- |
| P1 god store | ✅ 已拆 importer/preview（方案 A）+ taxonomy（hook 注册解耦，见下） | `stores/importer.ts`、`stores/preview.ts`、`stores/taxonomy.ts` |
| P2-1 导入重复 | ✅ 共享 `importBatch.runImportBatch` 状态机 | `web/src/importBatch.ts` |
| P2-2 Sidebar 复制 | ✅ 抽象 TaxonomyRow | `components/TaxonomyRow.vue` |
| P2-3 布局耦合 | ✅ layout.ts 单一来源 + CSS 变量 | `web/src/layout.ts` |
| P3 手势/布局不可单测 | ✅ useZoomPan + layoutRows 纯函数 | `composables/useZoomPan.ts`、`layout.spec.ts` |
| P4 缺少单测 | ✅ vitest 3 个纯逻辑 spec | `viewLogic/importBatch/layout.spec.ts` |
| P5 hawkShell 散点 | ✅ platform.ts 收敛 + no-op 壳 | `web/src/platform.ts` |
| P6 localStorage 键散落 | ✅ persist.ts STORAGE_KEYS 注册表 | `web/src/persist.ts` |
| P7 主进程手写 TOML | ✅ LAN 配置读写下沉 daemon（`GET/PUT /api/v1/app/lan`，toml_edit 保留注释），主进程手写解析已删除 | `hawk-daemon/src/api/lan.rs` |
| P8 大组件观察项 | ✅ 已拆（SettingsDialog 按分区子组件、Inspector 按选中态） | — |

P1 补记：taxonomy 拆分未采用本文档方案 B 的「返回值 + 组件层编排」，而是用 `registerTaxonomyHooks`
模块级钩子（taxonomy 创建时注册防抖刷新回调，主 store 经钩子转发、不反向 import）解决了预言中的
双向引用问题；restoreView 的存在性校验经 `init(validators)` 参数注入。

## 总体结论

前端架构**不是屎山，整体健康**。分层清晰（api → Pinia store → 组件，依赖单向）、契约先行
（TS 类型从 OpenAPI schema 生成，不手写对接口）、Electron 壳保持薄（业务零 IPC）、SSE 增量刷新
策略经过深思（防抖 + in-flight 合并 + 未过滤视图特判）、注释质量高（大量「为什么」级注释记录了
踩坑知识）。类型检查干净（`vue-tsc --noEmit` 0 错误），全库无 `any` 逃逸、无 `@ts-ignore`、
无 TODO/FIXME 残留。

但存在若干**正在积累的腐化风险**，按优先级列出。核心判断：单 store 正在长成 god store、
三处复制粘贴、手势/布局逻辑不可单测，是未来两年最大的退化源。

## 问题总览

| 编号 | 问题 | 级别 | 涉及文件 | 建议阶段 |
| ---- | ---- | ---- | -------- | -------- |
| P1 | `stores/library.ts` 正在长成 god store（1189 行） | 高 | `web/src/stores/library.ts` | 阶段 2 |
| P2-1 | `importPaths` 与 `importFiles` 约 90% 重复 | 中 | `web/src/stores/library.ts` | 阶段 1 |
| P2-2 | Sidebar 分类/标签两套代码逐行复制 | 中 | `web/src/components/Sidebar.vue` | 阶段 1 |
| P2-3 | ItemGrid 布局常量与 ItemCard CSS 隐式耦合 | 中 | `web/src/components/ItemGrid.vue`、`ItemCard.vue` | 阶段 1 |
| P3 | PreviewOverlay 手势引擎内联，齐行布局算法内联，均不可单测 | 中 | `web/src/components/PreviewOverlay.vue`、`ItemGrid.vue` | 阶段 3 |
| P4 | 缺少单元测试（Vitest 被列为 v1 非目标） | 中 | 全部（新增基础设施） | 阶段 0 |
| P5 | `window.hawkShell` 散点判断 44 处 / 16 文件 | 低 | 多个组件/composables | 阶段 1 |
| P6 | localStorage 键散落四处，无集中登记 | 低 | `App.vue`、`client.ts`、`library.ts` | 阶段 1 |
| P7 | 主进程手写 TOML 文本解析（已知妥协） | 低 | `electron/main.cjs` | 可选，随时 |
| P8 | 大组件观察项 | ✅ 已拆 | `Inspector.vue`、`SettingsDialog.vue`（Sidebar 481 行已收敛，维持） | — |

---

## P1：`stores/library.ts` 正在长成 god store（高）

### 现状证据

- `hawk-app/web/src/stores/library.ts` 共 1189 行，包含约 35 个 state ref、约 90 个函数/计算属性。
- 内部已有清晰分节（`// ---- state ----` 43 行起、getters 130、初始与查询 218、选择 541、
  item 写操作 571、多位置删除 636、导入重复策略 766、文件夹写操作 860、分类/标签 899、
  预览浮层 1050、SSE 1117），说明目前仍「宽而浅」、可读，但它是全项目**唯一**的聚合点，
  每加一个功能都会碰它。
- 覆盖域：视图/查询/列表数据/选择集/文件夹树/分类/标签/计数/缩略图偏好/搜索/预览/图片编辑/
  导入/回收站/批量操作/SSE 分发/视图历史/排序偏好/只读模式。

### 风险

- 继续增长到 2000 行后，单文件导航困难、每次改动 review 成本高、多人协作冲突加剧。
- 纯逻辑（导入状态机、SSE 策略、排序继承）埋在 store 里无法单测（与 P4 联动）。

### 改造方案（推荐方案 A，简单优先）

**方案 A：只拆独立性最强的两个域，其余保留。** 拆分后主 store 降至约 800 行，风险最低。

1. **`web/src/stores/importer.ts`（新）**：迁走导入域的全部状态与逻辑——
   `importProgress`、`dupPrompt`、`askDuplicatePolicy`、`resolveDuplicatePolicy`、
   `importBegin`、`importPaths`、`importFiles`。合并 P2-1 的重复代码（见下）。
   依赖：`api`、主 store 的 `currentFolderPath`（getter，只读）、`showToast`（action 调用）。
2. **`web/src/stores/preview.ts`（新）**：迁走预览/编辑域——`previewId`、`lastPreviewItem`、
   `previewItem`、`previewIndex`、`previewNavId`、`openPreview`/`closePreview`/`navigatePreview`、
   `editorTarget`、`openEditor`/`closeEditor`/`saveImageEdit`。
   依赖：`api`、主 store 的 `details`/`skeleton`（只读 getter）、`showToast`。
3. 其余状态（view/query/skeleton/details/selection/folders/categories/tagList/计数/历史栈/
   排序偏好/UI 开关）留在主 store——它们是列表核心，互相缠绕，拆了收益低。
4. 调用点更新：`App.vue`（导入进度浮层、PreviewOverlay/ImageEditDialog 挂载条件）、
   `useDragImport.ts`、`ItemGrid.vue`、`PreviewOverlay.vue`、`ImageEditDialog.vue`、
   `useShortcuts.ts`、`ImportDuplicateDialog.vue` 中对应的 `store.xxx` 改为子 store 调用。

**引用规则（防循环依赖，必须遵守）**：

- 引用方向必须构成 DAG（无环）。
- 子 store（importer/preview）可以 `useLibraryStore()` 读主 store 的 state/getter、
  调主 store 的 action；**主 store 不得反向引用子 store 的任何东西**。
- 子 store 之间不得互相引用。
- 跨 store 的编排流程（如 `init`、SSE `applyEvent` 分发）由组件层（App.vue）负责，
  不在 store 之间互相调用初始化逻辑。

**方案 B（仅当方案 A 完成后主 store 仍持续膨胀才考虑）**：继续拆 `taxonomy`（folders/categories/
tagList/计数 + CRUD）与 `selection`。注意 taxonomy 的 CRUD 依赖主 store 的 `correctView`
（当前视图跟随重命名/删除回退），会产生双向引用——届时需把「视图修正」改为返回值 +
组件层编排（如 Sidebar 调 `const renamed = await taxonomy.renameCategory(...)` 后自己调
主 store 的 `correctView`）。**本期不做方案 B。**

### 验收标准

- 主 store 不再包含 import/preview 域的任何状态与函数；行为与拆分前**完全一致**（纯重构，
  不改任何用户可见行为，不做兼容层）。
- `npm run build`、`npm run test:mobile` 通过；新增单测（P4 落地后）覆盖迁移的导入状态机。
- 文档同步：更新 hawk-app.md 目录结构一节，新增两个 store 的职责说明。

---

## P2-1：`importPaths` 与 `importFiles` 约 90% 重复（中）

### 现状证据

`hawk-app/web/src/stores/library.ts` 782–822 行（`importPaths`）与 823–856 行（`importFiles`）。
两者重复：重复策略状态机（`'ask' | 'skip' | 'import'`）、计数汇总（added/existed/skipped/failed）、
汇总 toast 文案、`importProgress` 推进。唯一差异是「单个文件导入调用」：
`api.itemAddByPath(path, {...})` vs `api.itemUpload(file, {...})`。

### 风险

- 改重复策略或进度逻辑要改两处，漏改即行为不一致。

### 改造方案

抽共享批量循环（放 `importer.ts`，与 P1 一起做；若 P1 延期可先独立做）：

```ts
// 伪代码：实现者可调整签名
async function runImportBatch<T>(
  items: T[],
  importOne: (item: T, skipExisting: boolean) => Promise<{ skipped: boolean; alreadyExisted: boolean }>,
): Promise<void>
```

- 内部实现现有状态机：空列表提示 → `importProgress` 初始化 → 逐项 `importOne` →
  `skipped && policy === 'ask'` 时弹 `askDuplicatePolicy()` 并以 `skipExisting: false` 重试 →
  计数 → done 推进 → 结束汇总 toast。
- `importPaths` / `importFiles` 各缩成约 5 行的适配函数（构造 `importOne` 回调、
  拼 `folder_path: currentFolderPath.value ?? undefined`）。
- 汇总文案「导入完成」vs「上传完成」作为参数传入。

### 验收标准

- 两个入口的行为与拆分前完全一致（含重复弹窗「整批生效」语义、进度条、toast 文案差异）。
- 单测覆盖状态机的 ask → skip / ask → import 两条路径（P4 落地后）。

---

## P2-2：Sidebar 分类/标签两套代码逐行复制（中）

### 现状证据

`hawk-app/web/src/components/Sidebar.vue`：

- 函数：`onCategoryRowContextMenu`（166–192 行）与 `onTagContextMenu`（195–221 行）结构相同
  （重命名/刷新缓存/删除，仅 API 与文案不同）。
- 模板：`.cat-row` 区块（315–331 行）与 `.tag-row` 区块（347–358 行）结构相同。
- 样式：`.tag-row*`（534–564 行）与 `.cat-row*`（566–599 行）逐条重复（仅选择器名不同）。

### 风险

- 加一个交互（如悬停菜单、双击重命名）要改两处；两套样式会逐渐漂移。

### 改造方案（二选一，选改动更小的）

1. **方案 i（推荐）**：抽 `TaxonomyRow.vue` 子组件，props：`kind: 'category' | 'tag'`、
   `name: string`、`count: number`、`active: boolean`、`dropTarget: boolean`。
   组件内部按 `kind` 选择图标/API/右键菜单项/删除文案，CSS 合并为一套 `.tax-row`。
   拖拽 drop 逻辑（Sidebar 现 96–155 行的 enter/leave/drop 处理）保持容器级委托不变，
   `rowKey` 用 `data-kind`/`data-name` 驱动。
2. **方案 ii**：不抽组件，只把右键菜单构造抽成泛型函数
   `taxonomyMenu(kind, name): MenuItem[]`，CSS 用 `.cat-row, .tag-row` 选择器合并。
   改动最小但模板重复仍在。

### 验收标准

- 分类/标签两行的显示、右键菜单、拖入高亮、计数与改造前一致。
- Sidebar.vue 从 614 行降到约 480 行以下（方案 i）。

---

## P2-3：ItemGrid 布局常量与 ItemCard CSS 隐式耦合（中）

### 现状证据

`hawk-app/web/src/components/ItemGrid.vue` 46–52 行：

```ts
const GAP = 10;
const META_H = 54;       // 注释明确要求与 ItemCard.vue 中 .meta 的 height 一致
const CARD_BORDER = 4;   // 卡片边框 2px×2
```

`hawk-app/web/src/components/ItemCard.vue` 132–134 行 `.meta { height: 54px }`、卡片边框 2px。
对齐靠注释，无编译期关联。改卡片样式（如 meta 高度）忘改 ItemGrid 会出「下一行盖住上一行
meta 文字」的布局 bug（该坑在注释中已被记录过）。

### 风险

- 脆弱：卡片样式与行布局数学脱钩，改一处漏一处即视觉损坏，且只在特定行高下暴露。

### 改造方案

1. 新建 `web/src/layout.ts`（与 P3 的布局纯函数同文件，避免文件碎片化），导出
   `GRID_GAP`、`CARD_META_H`、`CARD_BORDER`。
2. ItemGrid 从 layout.ts 导入，删除本地常量。
3. ItemCard 模板给卡片根元素绑定 CSS 变量，样式改用变量：
   ```html
   <div class="card" :style="{ '--meta-h': CARD_META_H + 'px', '--card-border': CARD_BORDER + 'px' }">
   ```
   `.meta { height: var(--meta-h, 54px) }`、`border: calc(var(--card-border, 4px) / 2) solid ...`
   （边框 CSS 写法以实现者为准，保持视觉不变即可）。
4. `GAP` 目前在 ItemGrid 模板硬编码 `gap: 10px`（`.row` 样式），同样改 `gap: var(--grid-gap)`。

### 验收标准

- 视觉零变化（网格行距/卡片边框/meta 高度与改造前像素一致）。
- 只改 layout.ts 一处即可全局生效；ItemGrid 中不再出现魔法数字注释「必须与 ItemCard 一致」。

---

## P3：手势引擎与布局算法内联，不可单测（中）

### 现状证据

- `web/src/components/PreviewOverlay.vue` 624 行，其中约 300 行（约 90–330 行）是手势状态机：
  捏合/平移/滑动切换/下拉关闭，含指针 Map、pinch 状态、moved 阈值、跨模式（缩放=1 翻页 ↔
  缩放>1 平移）语义切换。逻辑复杂且深埋组件内，无法单测、无法复用。
- `web/src/components/ItemGrid.vue` 60–160 行的齐行布局算法（贪心装行、非末行反推行高、
  0.5×–1.75× 夹紧、fitH 硬顶、末行规则）内联在 computed 中，同样是纯逻辑无法单测。

### 风险

- 这是全项目「最可能被改坏且最不易肉眼验证」的代码。任何手势/布局调整都靠真机手测。

### 改造方案

1. **布局算法抽纯函数**（先做，风险最低）：把 ItemGrid 的 `layout` computed 主体抽成
   `web/src/layout.ts` 的纯函数：
   ```ts
   export interface SkeletonLike { id: string; width: number; height: number; star: number }
   export interface LayoutRow { key: string; cells: {...}; y: number; height: number; startIdx: number; endIdx: number }
   export function layoutRows(
     sk: SkeletonLike[], containerWidth: number, targetH: number,
     opts?: { gap?: number; metaH?: number; cardBorder?: number },
   ): LayoutRow[]
   ```
   ItemGrid 的 computed 变薄调用。行为零变化。
2. **手势抽 composable**（后做，依赖单测兜底）：`web/src/composables/useZoomPan.ts`，
   接收一个全屏手势层元素与状态回调（`onScaleChange`/`onSwipe`/`onPullClose`/`onTap`/`onMoved`），
   内部管理 pointers/dragStart/pinch/moved 与模式切换。PreviewOverlay 只保留视觉层
   （imageStyle/trackStyle）与语义回调接线。抽取时以现有注释里的语义矩阵为规格，
   逐条核对：滚轮不动点缩放、双击语义、双指捏合、跨 scale=1 不丢跟踪、56px 滑动阈值、
   96px 下拉关闭、橡皮筋 0.35x、点击 vs 拖拽区分。
3. 两者均配合 P4 的单测交付（布局算法测试先行，手势至少覆盖状态机决策的纯部分）。

### 验收标准

- 行为零变化：桌面滚轮/拖拽、iPad 捏合/下拉关闭/ⓘ 详情条、手机滑动切换全部按原语义工作
  （真机手测清单由实现者列出并逐项打勾，`npm run test:mobile` 冒烟通过）。
- 布局算法单测覆盖：正常行、全景图宽行 fitH 硬顶、末行、0 宽高兜底（ratio=1）、
  行高夹紧上下限、空骨架。

---

## P4：缺少单元测试（中，先行基础设施）

### 现状证据

- 项目无 Vitest；`hawk-app.md`「非目标」一节明确「前端单元测试框架（Vitest 暂缓）」。
- 唯一自动化是 `hawk-app/scripts/test-mobile-web.mjs` 移动端冒烟（覆盖链路很薄：
  启动屏 → 网格渲染 → 双击预览 → 上传/删除）。

### 风险

- 上述全部重构（P1/P2/P3）没有回归网，纯靠手测。布局数学、SSE 策略、导入状态机、
  EXIF 处理都是高隐性 bug 区。

### 改造方案

1. 加 devDependency `vitest`，根 `package.json` 增脚本：
   `"test:unit": "vitest run"`（web/ 目录下 vitest 配置，测试文件放
   `web/src/**/*.spec.ts`，与源码同目录或 `__tests__/` 均可，实现者定并写入文档）。
2. 首期测试目标清单（优先级从高到低）：
   - **layout.ts 齐行布局**（P3-1 落地后）：见 P3 验收标准。
   - **导入状态机**（P2-1 落地后）：ask→skip / ask→import / 空列表 / 计数与 toast 文案。
   - **SSE 策略的纯决策逻辑**：从 library.ts 抽出的 `sameNameSet`（现有）、骨架 star/宽高
     合并判断（`applyUpdatedItem` 内）、`isUnfilteredView`、`viewPathPrefix`、`resolveSort`
     （folder 父链继承、category/tag 直落）、`select` 的 range/toggle 逻辑。
   - **imageEdit.ts** 的 EXIF 字节处理（Orientation 重置/回填）抽成纯函数后测字节级输出；
     canvas 编码部分不测（需浏览器 API）。
   - **store 集成**（可选）：`createTestingPinia` + mock api 层，覆盖 `resetList` 的
     `skeletonVersion` 过期响应丢弃、`ensureWindow` 区间补数、`reloadSkeleton` 的
     dirty 合并循环。
3. 不追求覆盖率数字；只测「纯函数 + 决策逻辑」，不测模板渲染。
4. CI（`.github/workflows/ci.yml`）的 build 步骤追加 `npm run test:unit`（可选，实现者评估）。

### 验收标准

- `npm run test:unit` 通过且纳入常规验证命令；README/hawk-app.md 的构建命令表同步。

---

## P5：`window.hawkShell` 散点判断 44 处 / 16 文件（低）

### 现状证据

全库 44 处 `window.hawkShell` 引用，16 个文件，三种形态并存：

- 能力调用：`window.hawkShell?.toggleMaximizeWindow()`（TitleBar/Sidebar/WindowControls 等）；
- 存在性判断：`const hasShell = !!window.hawkShell`（WindowControls/Sidebar/ItemGrid 等）；
- 平台判断：`window.hawkShell?.platform === 'darwin'`（StartingScreen/TitleBar/WindowControls）。

类型声明集中在 `web/src/types.ts` 的 `declare global`。`platform.ts` 已有部分收敛
（`platform`/`showInFileManagerLabel`）。

### 风险

- 平台分支矩阵（shell × narrow × touch）是本项目固有复杂度，散点判断使每处新增能力
  都要手写 `?.` 兜底，易漏写导致浏览器端报错。

### 改造方案

1. 扩展 `web/src/platform.ts`（不再新建文件）：
   ```ts
   export const hasShell = !!window.hawkShell;
   // 真实平台由 preload 注入，纯浏览器调试按 userAgent 兜底（与现有 platform 推导逻辑一致）
   export const isMac = platform === 'darwin';
   ```
2. 在 types.ts 的 `hawkShell` 类型基础上，platform.ts 导出类型化 no-op 壳：
   ```ts
   const noopShell: NonNullable<Window['hawkShell']> = { /* 每个方法返回类型化空值 */ };
   export const shell = window.hawkShell ?? noopShell;
   ```
   no-op 返回值语义：`listLibraries()` → `{ current: null, libraries: [] }`；
   `selectLibrary/openLibrary/toggleMaximizeWindow` → `false`；`getPathForFile` → `''`；
   订阅类（`onServerStarted` 等）→ 返回空退订函数；其余 `Promise<void>` → `Promise.resolve()`。
3. 组件改造：`window.hawkShell?.xxx()` → `shell.xxx()`（去 `?.`）；
   `!!window.hawkShell` → `hasShell`；`window.hawkShell?.platform === 'darwin'` → `isMac`。
   保留条件渲染语义不变（`hasShell` 仍用于 `v-if` 隐藏 Electron 专属 UI）。
4. 注意例外：
   - `useStartup.ts` 的 `if (!shell) return` 分支改为 `if (!hasShell) return`，轮询/订阅逻辑不变；
   - `useDragImport.ts` 的 `const shell = window.hawkShell; if (shell) {...} else {...}`
     改为 `if (hasShell) { shell.getPathForFile(...) }` 语义不变；
   - `App.vue`/`useStartup.ts` 里订阅函数的**返回值**（退订函数）语义必须保持。

### 验收标准

- 全库（web/src）不再出现 `window.hawkShell` 字样（types.ts 的 `declare global` 声明除外）；
- `npm run build` + `npm run test:mobile` 通过；Electron 端窗口控制/换库/设置热更、
  浏览器端只读查看行为均与改造前一致。

---

## P6：localStorage 键散落四处（低）

### 现状证据

| 键 | 读写位置 |
| -- | -------- |
| `hawk:panelWidths` | `App.vue` 74/90 行（load/savePanelWidths） |
| `hawk:thumbSize` | `stores/library.ts` 91/106 行（loadUserThumbSize/setUserThumbSize） |
| `hawk:lastView:<libPath>` | `stores/library.ts` 266/294 行（viewStorageKey/applyView） |
| `hawk:token:<host>` | `api/client.ts` 37/49/53 行（tokenStorageKey/storeToken/clearStoredToken） |

各处各自 try/catch 损坏值回退，逻辑重复。

### 风险

- 键命名无集中登记，未来加持久化容易撞名或格式漂移；损坏值回退策略各写各的。

### 改造方案

1. 新建 `web/src/persist.ts`：
   - 键注册表常量 + 每键 JSDoc（用途、格式、损坏回退策略）；
   - `loadJSON<T>(key, fallback: T): T`、`saveJSON(key, value)`：集中 try/catch
     （隐私模式写入失败静默、损坏值回退 fallback）；
   - 具体键的读写函数（如 `loadThumbSize()`、`viewStorageKey(libPath)`、`tokenStorageKey(host)`）
     迁移过来，调用点改为导入。
2. 保持键名与格式**不变**（用户已有数据不失效；不做迁移层，因为格式没变）。

### 验收标准

- 四个键的读写全部经 persist.ts；localStorage 键名字符串不再散落在 persist.ts 之外；
- 行为一致（损坏值回退、写入失败静默的语义保持）。

---

## P7：主进程手写 TOML 文本解析（低，可选）

### 现状证据

`hawk-app/electron/main.cjs` 354–382 行 `readWebSection`、383–411 行 `writeWebSection`：
文本级解析/写回 `.hawk/config.toml` 的 `[web]` 段（整段替换、其余段原样保留）。
注释已说明这是妥协（「TOML 由 server 权威解析」）。已知边缘情况：值内引号会被 strip
（`String(web.token).replace(/["\\]/g, '')`），token 含特殊字符时可能失真。

### 风险

- 目前 config.toml 只有 `[web]` 段被主进程文本级碰触，风险可控；但若未来主进程需要读写
  更多配置段，手写解析会持续积累边缘 bug。

### 改造方案（二选一）

1. **保持现状**（推荐短期）：在 readWebSection 上方补充注释明确边界「仅限 [web] 段、
   若需要读其他段必须换 TOML 库」，本问题关闭。
2. **换轻量 TOML 库**：引入 `smol-toml`（或等价轻量库，需兼容 CJS），readWebSection 改为
   全文件解析后取 `web` 字段；writeWebSection 需保持「整段替换、其余原样保留、键序固定」
   的语义（防止重写打乱用户手写注释/格式）。验收：新旧版本对同一 config.toml 的解析结果一致。

### 验收标准（选方案 2 时）

- 设置面板读写局域网配置与改造前一致；含特殊字符 token 的保存/读取 round-trip 正确；
- config.toml 中 [web] 段以外的内容逐字节不变。

---

## P8：大组件观察项（本期不动）

`Inspector.vue`（721 行）、`SettingsDialog.vue`（696 行）、`Sidebar.vue`（614 行，P2-2 后约 480）、
`App.vue`（353 行，混合启动状态机/栏宽拖拽/浮层挂载/全局 composable 接线）。

当前各自内聚尚可，拆分收益不明确，**不做**。触发再拆的条件（写进注释或文档留档）：
单文件再增 200 行以上、或新增功能需要理解两个以上不相关的现有区块。

---

## 实施顺序与阶段划分

依赖关系：P4（测试基础设施）先行兜底 → P2-1/P2-2/P2-3/P5/P6（低风险消重，互不依赖，
可并行）→ P1（store 拆分，依赖 P4）→ P3（手势/布局抽取，布局部分依赖 P4，手势部分最后做）。
P7 独立可选。

```
阶段 0  P4  Vitest 引入 + 现有纯函数（sameNameSet/resolveSort 等）首批测试
阶段 1  P2-1 导入状态机合并（若 P1 未做，先在本文件内合并）
        P2-2 Sidebar 分类/标签收敛（TaxonomyRow 方案 i）
        P2-3 layout.ts 常量收敛
        P5   useShell/hasShell/isMac 收敛（platform.ts）
        P6   persist.ts 收口
阶段 2  P1   importer/preview 子 store 拆分（吸收阶段 1 的 P2-1 成果）+ 单测
阶段 3  P3-1 布局算法抽纯函数 + 单测
        P3-2 useZoomPan 手势抽取 + 真机手测清单
```

每个问题独立提交（英文 commit message），便于 review 与回滚。

## 全局约束（来自 AGENTS.md，实施者必读）

1. **纯重构，行为零变化**：以上所有条目不改变任何用户可见行为；不做兼容层、不做回退机制，
   废弃代码直接移除。
2. **简单优先**：每个问题取满足需求的最小改动方案；不引入无实际需求的抽象/配置项。
3. **文档同步**：完成任一问题后同步更新 [hawk-app.md](../frontend/hawk-app.md) 受影响章节
   （目录结构、组件契约表、composables 表、Vue 实践基线、非目标中的测试条目）与
   README 构建命令表（新增 `npm run test:unit` 时）。
4. **代码/commit 用英文，注释与文档用中文**。
5. **验证命令**（每次提交前）：
   ```bash
   cd hawk-app
   npm run build          # vue-tsc --noEmit -p web/tsconfig.json && vite build
   npm run test:unit      # P4 落地后
   npm run test:mobile    # 移动端冒烟（需先构建 hawk-daemon，见 hawk-app/README.md）
   ```
6. 不确定的行为差异：以现有注释与 hawk-app.md 中的语义描述为准，无法确认时标 `[UNCERTAIN]`
   并给出验证方式，不要猜测实现。

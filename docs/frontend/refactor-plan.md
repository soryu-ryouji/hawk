# 前端架构重构方案（长期基座：feature 目录 + 域公共出口 + 边界强制 + 分层可测）

状态：**已确认（方案 C）**。前提释义：hawk 前端是**长期持续拓展的基座**——路线图上的远程查看、移动端、服务器版都会持续往它身上加功能，边界投资有明确的近期买家（非克隆模板、非范例展示）。

四个要求对应的落地：

| 要求 | 落地为 |
| --- | --- |
| 最优美 | 单一代码路径、无死层、数据流可从目录读出——**不以层数与间接层数衡量优美**，方案明确克制（见第 6 节） |
| 最工程 | 依赖规则机器强制（CI 红线），不靠约定与记忆 |
| 最容易长期维护 | 域 `index.ts` 公共出口——域内重构永不外溢 |
| 每一步方便测试 | 分层可测架构（第 4 节）：每层有既定测试形态，每阶段提交时点全量 spec 绿、历史可 bisect |

## 1. 目标与核心价值

长期基座对架构的要求：**任何域的内部重构都不应外溢**。四个机制：

1. **按域组织**（feature-first）：每个业务域一个目录，内含自己的 store / 服务 / 组件 / 逻辑 / 测试；
2. **域公共出口**：每个域只经 `index.ts` 对外暴露白名单成员，域内文件对外不可见；
3. **机器强制的依赖规则**：eslint 约束「域之间只准 import 对方的 index.ts、只准向下引用 shared」——违规在 CI 报错，不靠约定；
4. **分层可测**：域内强制三层（logic / store / components），每层有对应的测试形态（见第 4 节）——新增代码必须落入某一层，不允许出现既非纯函数、又非 store、又非组件的漂浮逻辑（现状根目录散落的 `*.ts` 正是第四种形态）。

## 2. 模式出处与备选对比

| 备选 | 出处 | 判断 |
| --- | --- | --- |
| **feature-first 域目录**（本方案） | Vue 官方文档规模化指引（较大的应用按 feature 组织目录）；Pinia 官方文档「按领域概念定义 store」；大型 Vue 应用（如 GitLab 前端按 feature 目录组织）的主流收敛 | **采用**。与单窗口桌面应用形态匹配 |
| Feature-Sliced Design（FSD） | feature-sliced.design，正式方法论，有 eslint 配套 | **不采用**：FSD 的 pages/widgets/features/entities 分层面向多页面 Web 应用，hawk 是单窗口桌面工具（pages=1），强行套用会产生空层仪式感 |
| type-first（现状：components/ stores/ composables/ 平铺） | 小型应用惯例 | 规模到 77 文件后边界靠人脑记忆，是本次要替换的对象 |

本方案同时保留此前已确认的机制（服务层、特征化测试、composable 抽取），落位到域内。

## 3. 目标结构

```text
web/src/
├── main.ts
├── app/                          # 装配层：唯一允许引用所有域的位置
│   ├── App.vue                   # 相位分发 + 布局模板
│   ├── boot.ts                   # useBoot：启动相位机 + SSE 桥 + 重启监听
│   ├── panelResize.ts            # usePanelResize
│   ├── shortcuts.ts              # useShortcuts（跨域快捷键编排）
│   └── screens/                  # StartingScreen / SetupScreen / ConnectScreen
│       └── chrome/               # TitleBar / WindowControls（应用铬件，内嵌库控件经 library 出口）
├── shared/                       # 无业务依赖的原语，任何层可引用
│   ├── api/                      # client / endpoints / events / schema.d.ts（生成物）
│   ├── types.ts                  # 契约 re-export（schema + IPC）
│   ├── ui/                       # Icon / EmptyState / PromptDialog / SelectBox / ContextMenu
│   ├── lib/                      # format / persist / platform / clipboard / saveImage
│   └── composables/              # useLayout / useLongPress / useContextMenu（通用交互）
└── domains/
    ├── library/                  # 素材库域（核心）
    │   ├── index.ts              # 公共出口：useLibraryStore、写操作服务、类型
    │   ├── store.ts              # 状态 + 查询/窗口 + 选择集 + applyEvent（~590 行）
    │   ├── actions.ts            # 写操作服务层（~280 行）
    │   ├── logic/                # viewLogic / layout（齐行装行）/ useGridNav —— 纯函数层
    │   └── components/           # ItemGrid / ItemCard / FilterBar / SearchBox / SizeMenu /
    │                             # Inspector / InspectorItem / InspectorBatch / StarRating
    ├── taxonomy/
    │   ├── index.ts / store.ts
    │   └── components/           # Sidebar / FolderTreeNode / FolderTreePicker /
    │                             # FolderPickerDialog / TaxonomyRow / TagEditor / CategoryPickerDialog
    ├── preview/
    │   ├── index.ts / store.ts
    │   ├── logic/                # useZoomPan / imageEdit
    │   └── components/           # PreviewOverlay / ImageEditDialog
    ├── import/
    │   ├── index.ts / store.ts   # importer store
    │   ├── logic/                # useDragImport / dnd / importBatch
    │   └── components/           # ImportDuplicateDialog
    └── settings/
        ├── index.ts
        ├── updater.ts            # useUpdater
        └── components/           # SettingsDialog / Settings{Appearance,Connection,Hiding,Lan,Storage,Update}
```

**依赖规则**（eslint 强制，见第 4b 节）：

```text
app ──► domains/*（只经 index.ts）──► shared
                └── 域之间禁止互相 import（跨域编排只发生在 app 层）
```

spec 文件随模块就近放置。

## 4. 可测试性架构（「每一步方便测试」的机制化）

### 4a. 分层与测试形态

每层有且只有一种测试方式，全部沿用仓库既有 spec 先例（不引入新测试栈）：

| 层 | 形态 | 测试方式 | 仓库先例 |
| --- | --- | --- | --- |
| `domains/*/logic/` | 纯函数 | 直接单测 | viewLogic.spec / layout.spec / importBatch.spec |
| `domains/*/store.ts` | Pinia store | `setActivePinia` + `vi.mock(shared/api/endpoints)`，无 DOM 依赖 | library.spec / libraryNavigation 的固化测试 |
| `domains/*/components/` | props 入 / 事件出 | `@vue/test-utils` 直接挂载（不依赖 store 的组件零 mock；依赖 store 的经 pinia 激活挂载，不强求 mock store） | SizeMenu.spec / itemCard.spec / sidebarRows.spec |
| `app/` 编排层 | composable | mock `shared/platform`（shell/IPC 适配器）与域出口函数 | boot.spec（本方案新增） |

配套规则：

- **api mock 恒指向单一稳定路径** `shared/api/endpoints`——阶段 1 把 api 迁入 shared 后，所有 `vi.mock` 目标一次改齐；此后任何域迁移都不再动 mock 路径（域文件改的是 import，不是 mock 目标）
- **薄组件、厚 logic**：判定逻辑下沉 logic/ 纯函数，组件只做绑定——这是组件层「零 mock 可挂载」的前提，SizeMenu 已是范式
- **shell/IPC 只经 `shared/platform.ts` 适配**：app 层可测性的前提（现状已是如此，固化为规则）

### 4b. 每阶段的测试门槛（与第 5 节对应）

- **每阶段提交时点：`npm run build && npm run test:unit && npm run lint` 全绿**——历史可 bisect；
- 阶段内禁止「移动 + 改逻辑」混合提交：移动提交不改任何断言，逻辑提交不移动任何文件；
- vitest 按文件名通配发现 spec（`**/*.spec.ts`），目录迁移无需改配置；`@vitest-environment jsdom` 注释随文件走。

## 5. 分阶段迁移

| 阶段 | 内容 | 测试产物 / 门槛 |
| --- | --- | --- |
| 0 | 特征化测试：SSE 分支（就地清理顺序/防抖重载/hooks 转发）、加载竞争（版本丢弃/in-flight 合并）、写操作消息组装 | 新增 `stores/library.behavior.spec.ts`，全绿后才开始移动文件 |
| 1 | 建 `shared/`：api、types、format、persist、platform、通用 ui 与 composables 迁入 | 纯移动；所有 spec 的 `vi.mock` 路径一次改到 `shared/api`，断言不变，全绿 |
| 2 | 建 `app/`：boot.ts（相位机 + SSE 桥）、panelResize.ts、screens 与铬件迁入 | 新增 `boot.spec`：UNAUTHORIZED→connect、serverRestarting→starting、浏览器路径轮询触发（mock platform + 域出口） |
| 3 | 建 `domains/library`：组件/逻辑迁入 + store 拆分（写操作 → actions.ts 服务层，store 减 16 函数增 2 个一行方法）+ index.ts 出口 | 特征化测试 SSE/竞争部分**不改一字**通过；写操作测试仅改调用来源；spec 随域就近 |
| 4 | 建 `domains/{taxonomy,preview,import}` | 纯移动，断言不变 |
| 5 | 建 `domains/settings` + eslint boundaries 从 warn 收敛为 error + 文档同步 | boundaries 规则全量生效，CI 守门 |

每阶段另做一次手动冒烟（浏览/搜索/批量操作/回收站/导入/SSE 对齐/窄屏/换库）。**任何阶段可以停住，代码库处于可发布状态。**

## 6. 明确不做的事与理由（优美的克制面）

- **不把 library store 再拆为「查询 store + 窗口 store + 选择集 store」**：三者在 `select/ensureWindow/reloadSkeleton` 上真实耦合，拆开必然互相引用。彻底性来自边界与出口，不来自切碎内聚状态
- **不为「工程感」增加仪式**：不引 DI 容器、不做 CQRS 式读写分离分层、不建空壳 store——间接层不是工程性的证明，删除特例才是（updateItem 与 SSE 走同一 `applyEvent` 入口即本方案的「品味」示范）
- 不迁 `libraryNavigation` factory（能工作、有测试；收进域内经 index.ts 隔离后，私有约定无害）
- 不引 vue-router / 组件库 / 状态库；不做 FSD
- electron 主进程不动（13 文件、职责清晰；其测试空白另案处理）

## 7. 风险与缓解

| 风险 | 缓解 |
| --- | --- |
| 大范围移动 import 断链 | 分五阶段、每阶段 vue-tsc 全量兜底；移动与逻辑修改分离提交 |
| 域归属的灰色文件（如 TitleBar 内嵌库控件） | 归属原则：**渲染谁的状态就归谁；跨域编排归 app**；第 3 节映射表已逐文件落位，评审时可改 |
| eslint boundaries 误伤既有合法引用 | 阶段 3 起以 warn 模式运行，阶段 5 收敛为 error——强制从教训里长出来，不预付 |
| vitest spec 发现遗漏（新目录） | 按文件名通配发现，阶段验收含「spec 数量前后一致」核对 |
| 未来形态突变（多页面 Web） | 第 2 节注明届时评估 FSD；域目录结构本身可平移 |

## 8. 执行后的文档同步

- `docs/frontend/hawk-app.md`：「目录结构」「Pinia store」章节按目标结构重写，新增「域边界与依赖规则」「分层测试约定」两节
- `hawk-app/README.md` 开发调试一节补目录导览
- `eslint.config.mjs` 注释写明规则意图

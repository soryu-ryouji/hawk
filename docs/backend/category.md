# Category 虚拟分类维度设计

## 动机与三维模型

hawk 的组织维度划分为三个：

| 维度 | 性质 | 对应物 | 一对多关系 |
| ---- | ---- | ------ | ---------- |
| Folder | 实体存储 | 磁盘上的真实目录 | 一个文件实体只在一个文件夹 |
| Category | **虚拟分类** | 元数据中的分类名字 | 一个 item 可属多个分类 |
| Tag | 虚拟标签 | 元数据中的扁平标签 | 一个 item 可有多个标签 |

动机：同一张图想同时出现在「海报」和「灵感参考」里时，不必把文件复制到多个实体文件夹——实体位置唯一，虚拟分类任意挂。

## Category 与 Tag 的定位差异

- **Category**：**扁平受控词表**，需先创建（可空挂），数量少、精心维护，相当于「不收实体的虚拟文件夹」
- **Tag**：扁平、数量多、随手打，适合检索关键词

> 历史版本曾支持层级分类路径（`插画/人物`），实践下来增加操作与理解负担，已移除：分类与标签同为扁平名字，仅「受控/自由」之别。

## 数据模型与存储

### item 元数据 `categories` 字段

```toml
# .hawk/metadata/<hash>.toml
categories = ["海报", "灵感参考"]   # 分类名字数组（扁平，可多选）
tags = ["nature"]
...
```

- categories 属于 item（内容级），与 tags/star 同级；参与同步
- 回收站中的 item 保留分类；清空回收站随元数据一起消失
- 每 item 分类数不设上限

### 注册表：支持空分类与空标签

分类与标签各有一个注册表文件，**先建后放**（空名字持久化）与**赋值即创建**（item/update 自动登记）两条路径并存：

```text
.hawk/
├── categories.toml   ← 分类注册表（参与同步）
├── tags.toml         ← 标签注册表（参与同步）
├── metadata/ ...
```

```toml
# categories.toml：扁平名字列表
categories = ["海报", "灵感参考"]
```

```toml
# tags.toml
tags = ["nature", "sunset"]
```

- 分类列表 = 注册表 ∪ 全部 item 赋值的并集；标签列表同理
- 注册表是中心文件，存在网盘同步冲突的可能——内容只是名字列表，冲突代价低，接受

### 命名规则

- 分类名称 trim、去重，不允许空串、不允许含 `/` 或 `\`（层级已废弃）；大小写敏感
- 标签名称 trim、去重，不允许空串

### 重命名与删除（批量元数据迁移）

| 操作 | 行为 |
| ---- | ---- |
| 分类重命名 | 注册表更名 + 全部命中 item 的 categories 替换；目标已存在时合并（分类是集合语义） |
| 分类删除 | 注册表移除 + 全部 item 的该分类赋值清除 |
| 标签重命名 | 注册表更名 + 全部 item 的 tags 替换 |
| 标签删除 | 注册表移除 + 全部 item 的该标签清除 |

批量迁移经索引流水线单写者执行，逐 item 保存元数据并推送 `item.updated`。

## API 变更（v1）

### Item 对象与查询

| 变更 | 说明 |
| ---- | ---- |
| Item 增加 `categories: string[]` | 分类名字列表 |
| `item/add` 增加 `categories?: string[]` | 初始分类（浏览器拖拽收集用；名称校验，整体替换语义） |
| `item/update` 增加 `categories?: string[]` | 整体替换（自动登记注册表） |
| `item/list` 增加 `categories?: string[]` | 分类过滤（精确匹配）；`categories_match`：`any`（默认，命中任一）/ `all`（全部命中） |
| `item/list` 增加 `exclude_categories?: string[]` | 排除：任一命中即剔除 |
| `item/list` 增加 `exclude_tags?: string[]` | 排除：任一命中即剔除（`tags` 参数维持 AND 语义不变） |

### category / tag 端点

| 方法 | 端点 | 说明 |
| ---- | ---- | ---- |
| GET | `/api/v1/category/list` | `[{ name, count }]`（注册表 ∪ 赋值并集），count 为库内（不含回收站）item 数 |
| POST | `/api/v1/category/create` | `{ name }` |
| POST | `/api/v1/category/update` | `{ name, new_name }` 重命名；目标已存在时合并 |
| POST | `/api/v1/category/delete` | `{ name }` |
| GET | `/api/v1/tag/list` | `[{ name, count }]`，count 为库内（不含回收站）item 数 |
| POST | `/api/v1/tag/create` | `{ name }` |
| POST | `/api/v1/tag/update` | `{ name, new_name }` |
| POST | `/api/v1/tag/delete` | `{ name }` |

新增错误码：`CATEGORY_NOT_FOUND`、`TAG_NOT_FOUND`、`CATEGORY_EXISTS`（目标名称已存在）。

### 事件

不新增事件类型：分类/标签结构变更后前端防抖重拉 `category/list` 与 `tag/list`（与文件夹树同款缺口，见 hawk-app.md 已知缺口）。

## UI（hawk-app）

```text
Sidebar
  全部素材 / 根目录素材 / 未分类素材 / 未标签素材 / 回收站
  文件夹 ── 实体存储（树）
  分类   ── 扁平列表（＋新建；右键：重命名/删除）
  标签   ── 标签列表（＋新建；右键：重命名/删除）
```

- 视图状态 `{ kind: 'category', name }` 与 `{ kind: 'tag', name }`，复用 ItemGrid
- 检查器：「分类」chip 编辑器；「＋」打开分类选择对话框（可选已有，也可输入新名字）
- 右键菜单：「添加到分类…」；「添加标签…」对话框带出已有标签自动补全（datalist）
- 浏览器插件拖拽收集：扫过分类可多选，投到文件夹/根目录/分类行，面板内可新建（见 hawk-browser-extension/README.md）

## 里程碑

| 阶段 | 产出 |
| ---- | ---- |
| C1 后端 | categories 元数据字段、注册表、查询过滤、category/tag 端点、批量迁移、单元测试 + smoke |
| C2 前端 | 侧栏分类/标签列表、分类与标签视图、检查器分类编辑、右键添加到分类 |
| C3 | 浏览器插件拖拽收集（多分类、面板内新建）、文档同步 |

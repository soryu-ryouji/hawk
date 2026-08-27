# Category 虚拟分类维度设计

## 动机与三维模型

hawk 的组织维度划分为三个：

| 维度 | 性质 | 对应物 | 一对多关系 |
| ---- | ---- | ------ | ---------- |
| Folder | 实体存储 | 磁盘上的真实目录 | 一个文件实体只在一个文件夹 |
| Category | **虚拟分类**（本次新增） | 元数据中的分类路径 | 一个 item 可属多个分类 |
| Tag | 虚拟标签 | 元数据中的扁平标签 | 一个 item 可有多个标签 |

动机：同一张图想同时出现在「海报」和「灵感参考」里时，不必把文件复制到多个实体文件夹——实体位置唯一，虚拟分类任意挂。

## Category 与 Tag 的定位差异

- **Category**：**层级树**（如 `插画/人物`、`插画/场景`），数量少、精心维护，相当于「不收实体的虚拟文件夹树」
- **Tag**：扁平、数量多、随手打，适合检索关键词

## 数据模型与存储

### item 元数据增加 `categories` 字段

```toml
# .hawk/metadata/<hash>.toml
categories = ["插画/人物", "参考/构图"]   # 分类路径数组，正斜杠分隔层级
tags = ["nature"]
...
```

- categories 属于 item（内容级），与 tags/star 同级；参与同步
- 回收站中的 item 保留分类；清空回收站随元数据一起消失
- 每 item 分类数不设上限

### 注册表：支持空分类与空标签

分类与标签各有一个注册表文件，**先建后放**（空节点持久化）与**赋值即创建**（item/update 自动登记）两条路径并存：

```text
.hawk/
├── categories.toml   ← 分类注册表（参与同步）
├── tags.toml         ← 标签注册表（参与同步）
├── metadata/ ...
```

```toml
# categories.toml：扁平路径列表，树由路径派生
categories = ["插画", "插画/人物", "参考/构图"]
```

```toml
# tags.toml
tags = ["nature", "sunset"]
```

- 分类树 = 注册表 ∪ 全部 item 赋值的并集；标签列表同理
- 注册表是中心文件，存在网盘同步冲突的可能——内容只是名字列表，冲突代价低，接受
- 创建多层分类（如 `插画/人物`）自动补齐祖先节点

### 命名规则

- 分类层级以 `/` 分隔；单段名称不允许 `/`、`\`、`.`、`..`、空白首尾；大小写敏感、去重
- 标签名称 trim、去重，不允许空串

### 重命名与删除（批量元数据迁移）

| 操作 | 行为 |
| ---- | ---- |
| 分类重命名/移动 | 注册表路径迁移 + 全部命中 item 的 categories 前缀迁移；**子树跟随**（`插画`→`灵感` 则 `插画/人物`→`灵感/人物`） |
| 分类删除 | 删除该节点及其子树：注册表移除 + 全部 item 的相关赋值清除 |
| 标签重命名 | 注册表更名 + 全部 item 的 tags 替换 |
| 标签删除 | 注册表移除 + 全部 item 的该标签清除 |

批量迁移经索引流水线单写者执行，逐 item 保存元数据并推送 `item.updated`。

## API 变更（v1）

### Item 对象与查询

| 变更 | 说明 |
| ---- | ---- |
| Item 增加 `categories: string[]` | 分类路径列表 |
| `item/update` 增加 `categories?: string[]` | 整体替换（自动登记注册表） |
| `item/list` 增加 `categories?: string[]` | 分类过滤，含子分类；`categories_match`：`any`（默认，命中任一）/ `all`（全部命中） |
| `item/list` 增加 `exclude_categories?: string[]` | 排除：任一命中（含子分类）即剔除 |
| `item/list` 增加 `exclude_tags?: string[]` | 排除：任一命中即剔除（`tags` 参数维持 AND 语义不变） |

### category / tag 端点

| 方法 | 端点 | 说明 |
| ---- | ---- | ---- |
| GET | `/api/v1/category/list` | 分类树（注册表 ∪ 赋值并集）；节点 path/name/children |
| POST | `/api/v1/category/create` | `{ path }`，自动补齐祖先 |
| POST | `/api/v1/category/update` | `{ path, name?, parent_path? }`，重命名/移动，子树跟随 |
| POST | `/api/v1/category/delete` | `{ path }`，删除节点及子树 |
| GET | `/api/v1/tag/list` | `[{ name, count }]`，count 为库内（不含回收站）item 数 |
| POST | `/api/v1/tag/create` | `{ name }` |
| POST | `/api/v1/tag/update` | `{ name, new_name }` |
| POST | `/api/v1/tag/delete` | `{ name }` |

新增错误码：`CATEGORY_NOT_FOUND`、`TAG_NOT_FOUND`、`CATEGORY_EXISTS`（目标路径已存在）。

### 事件

不新增事件类型：分类/标签结构变更后前端防抖重拉 `category/list` 与 `tag/list`（与文件夹树同款缺口，见 hawk-app.md 已知缺口）。

## UI 变更（hawk-app）

```text
Sidebar
  全部素材
  文件夹 ── 实体存储（现状）
  分类   ── 分类树（＋新建；节点右键：新建子分类/重命名/删除）
  标签   ── 标签列表（＋新建；右键：重命名/删除）
  回收站
```

- 视图状态新增 `{ kind: 'category', path }` 与 `{ kind: 'tag', name }`，复用 ItemGrid
- 检查器：标签编辑器下方加「分类」chip 编辑器；「＋」打开分类选择对话框（可选已有路径，也可输入新路径）
- 右键菜单：「添加到分类…」；「添加标签…」对话框带出已有标签自动补全（datalist）
- 拖拽赋值（拖到分类树节点）列入后续，不进 v1

## 兼容与迁移

- 旧元数据无 `categories` 字段 → 解析为空数组，天然向后兼容
- 注册表文件不存在 → 视为空注册表，首次操作时创建
- OpenAPI schema 变更后 `npm run gen:types` 重新生成前端类型

## 里程碑

| 阶段 | 产出 |
| ---- | ---- |
| C1 后端 | categories 元数据字段、注册表、查询过滤、category/tag 八端点、批量迁移、单元测试 + smoke |
| C2 前端 | 侧栏分类树/标签列表、分类与标签视图、检查器分类编辑、右键添加到分类 |
| C3 | ui-check 扩展断言、文档同步 |

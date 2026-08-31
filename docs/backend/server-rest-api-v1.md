# Server REST API V1

## 通用约定

### 请求与响应

- 所有端点前缀 `/api/v1`
- 请求参数与响应字段均使用 snake_case
- 响应统一信封：

```json
{ "status": "success", "data": {} }
```

- 错误响应（HTTP 状态码同时为 4xx/5xx）：

```json
{
  "status": "error",
  "error": { "code": "ITEM_NOT_FOUND", "message": "item abc123 not found" }
}
```

错误码：`INVALID_PARAM`、`ITEM_NOT_FOUND`、`FOLDER_NOT_FOUND`、`FILE_EXISTS`、`UNSUPPORTED_FORMAT`、`CATEGORY_NOT_FOUND`、`CATEGORY_EXISTS`、`TAG_NOT_FOUND`、`INTERNAL`、`READ_ONLY`（viewer token 访问写端点，403）

### ID 规范

- **item id**：文件内容的 BLAKE3 哈希（hex），与存储设计一致
- **library**：不使用合成 id，以素材库根目录路径区分；显示名取 `config.toml` 的 `name`，缺省为库目录名
- **folder**：不使用合成 id，直接以相对素材库根目录的真实目录路径标识（如 `posters/2024`）

### 其他

- 时间戳均为 Unix 毫秒
- 分页参数：`offset`（默认 0）、`limit`（默认 50）
- 桌面版默认监听 `27371` 端口（被占用时回退为动态分配），所有请求需携带启动时下发的 token（`Authorization: Bearer <token>`）；SSE 与 `<img>` 直链（thumbnail、file）无法设置请求头，改用查询参数 `?token=`

## app

| 方法 | 端点               | 说明         |
| ---- | ------------------ | ------------ |
| GET  | `/api/v1/app/info` | 获取应用信息 |
| GET  | `/api/v1/app/startup` | 启动状态与索引构建进度（就绪网关唯一放行端点） |
| GET  | `/api/v1/app/status` | 后台任务积压（缩略图队列 + 索引管道；SSE 的 `task.progress` 事件为同一快照的推送版） |
| GET  | `/api/v1/app/token` | 发现连接 token（免鉴权，仅限扩展类客户端） |

### startup

`GET /api/v1/app/startup`

查询启动状态。server 先监听端口、初始索引后台构建；客户端轮询此端点获取进度（200ms 左右间隔为宜），也是索引完成前唯一可用的 API。

#### 响应

索引构建中：

```json
{
  "status": "success",
  "data": { "status": "starting", "phase": "hash", "processed": 343, "total": 800 }
}
```

就绪：

```json
{ "status": "success", "data": { "status": "ready" } }
```

初始索引失败（进程不退出，修复后需重启）：

```json
{ "status": "success", "data": { "status": "error", "message": "..." } }
```

| 字段 | 类型 | 说明 |
| ---- | ---- | ---- |
| status | string | `starting` / `ready` / `error` |
| phase | string | 仅 starting 时存在：`scan`（遍历清单，total 恒 0）/ `hash`（并行算哈希）/ `apply`（应用索引）/ `sync`（元数据对账，初始扫描前，total 恒 0） |
| processed / total | number | 仅 starting 时存在；`total=0` 表示该阶段总量未知（客户端宜显示不定态进度） |
| message | string | 仅 error 时存在：失败原因 |

### status

`GET /api/v1/app/status`

后台任务积压快照：缩略图（含调色板）worker 队列与索引管道。导入或初次索引后大量素材等待生成缩略图时，`thumbnail.pending`（排队中）与 `thumbnail.active`（生成中，并发度 `CPU/4`、封顶 8）大于 0；批量文件入库期间 `index.pending`（管道排队 job + 写入防抖路径）大于 0，扫描期间 `index.active` 为 1 并携带阶段进度。积压清空后归零。

SSE 客户端建议直接订阅 `task.progress` 事件（同一快照的推送版，服务端 500ms 节流）；本端点供轮询型客户端使用。

#### 响应

```json
{
  "status": "success",
  "data": {
    "thumbnail": { "pending": 236, "active": 4 },
    "index": { "pending": 12, "active": 0, "phase": "hash", "processed": 1800, "total": 4200 }
  }
}
```

`index.phase` / `processed` / `total` 仅扫描进行中存在：`total=0` 的遍历阶段表示总量未知（客户端宜显示不定态进度）；空闲时为 `null`。

### info

`GET /api/v1/app/info`

获取当前 hawk-server 的运行信息，可用于判断客户端环境能力。

#### 响应

```json
{
  "status": "success",
  "data": {
    "version": "1.0.0",
    "platform": "windows",
    "exec_path": "C:/Tools/hawk/hawk-server.exe",
    "access": "admin"
  }
}
```

| 字段      | 类型   | 说明                          |
| --------- | ------ | ----------------------------- |
| version   | string | 后端版本号                    |
| platform  | string | `windows` / `macos` / `linux` |
| exec_path | string | 后端可执行文件路径            |
| access    | string | 当前 token 的访问级别：`admin`（桌面端全权）/ `viewer`（局域网 web 查看只读 token，见 storage.md 的 `[web]` 配置） |

### health

`GET /health`

就绪探活，不带 `/api/v1` 前缀，无需 token。**初始索引完成前返回 503**，完成后 200。只用于区分进程状态，进度与就绪判断以 `app/startup` 为准。

### token

`GET /api/v1/app/token`

免鉴权的 token 发现端点，供浏览器插件等生态客户端零配置接入（插件端实现见 hawk-browser-extension）。

安全性约束：

- 响应**不携带 CORS 头**（`DisableCors`）：跨源网页 JS 无法读取响应，只有持 `host_permissions` 的浏览器扩展能读
- **Host 必须是环回地址**（`127.0.0.1` / `localhost` / `::1`），否则返回 `INVALID_HOST`：防 DNS rebinding 伪装同源读取

拿到 token 后，其余 `/api/*` 请求照常 `Authorization: Bearer <token>`。

#### 响应

```json
{ "status": "success", "data": "<random-token>" }
```

## library

hawk-server 单实例对应单个素材库。

| 方法 | 端点                      | 说明               |
| ---- | ------------------------- | ------------------ |
| GET  | `/api/v1/library/info`    | 获取当前素材库信息 |
| POST | `/api/v1/library/reindex` | 全量重建索引       |

### info

`GET /api/v1/library/info`

获取当前打开的素材库信息。文件夹树通过 `folder/list` 获取。

#### 响应

```json
{
  "status": "success",
  "data": {
    "name": "设计素材库",
    "path": "D:/Assets/Design",
    "modification_time": 1592461625783,
    "application_version": "1.0.0",
    "thumbnail_sizes": [256, 512, 1024]
  }
}
```

`thumbnail_sizes` 为缩略图尺寸白名单（来自项目配置，见 storage.md），前端网格据此构建 `srcset` 候选。

### reindex

`POST /api/v1/library/reindex`

全量重建索引：重新扫描素材目录并对所有文件重算哈希。异步执行，立即返回；过程中的变更照常通过 `events` 推送。用于 hawk 未运行期间直接改动过素材目录、或对索引状态存疑时手动触发。

#### 响应

```json
{ "status": "success" }
```

## folder

folder 即素材库中的真实目录。对 folder 的操作会直接操作文件系统，由文件监听同步到索引。

| 方法 | 端点                     | 说明         |
| ---- | ------------------------ | ------------ |
| GET  | `/api/v1/folder/list`    | 列出文件夹树 |
| POST | `/api/v1/folder/create`  | 创建文件夹   |
| POST | `/api/v1/folder/update`  | 更新文件夹   |
| POST | `/api/v1/folder/delete`  | 删除文件夹   |
| POST | `/api/v1/folder/restore` | 恢复文件夹   |

### list

`GET /api/v1/folder/list`

返回完整文件夹树。节点字段：`path`（相对库根目录）、`name`、`children`、`modification_time`。

### create

`POST /api/v1/folder/create`

#### 请求

| 参数        | 类型   | 必填 | 说明                                         |
| ----------- | ------ | ---- | -------------------------------------------- |
| name        | string | 是   | 文件夹名称                                   |
| parent_path | string | 否   | 父文件夹路径（相对库根目录），缺省为库根目录 |

#### 响应

```json
{
  "status": "success",
  "data": {
    "path": "posters/2024",
    "name": "2024",
    "children": [],
    "modification_time": 1592409993367
  }
}
```

### update

`POST /api/v1/folder/update`

#### 请求

| 参数        | 类型   | 必填 | 说明                           |
| ----------- | ------ | ---- | ------------------------------ |
| path        | string | 是   | 文件夹路径                     |
| name        | string | 否   | 新名称（重命名即移动真实目录） |
| parent_path | string | 否   | 新父目录路径（移动真实目录）   |

#### 响应

同 `folder/create`。

### delete

`POST /api/v1/folder/delete`

删除文件夹：将目录（含其中素材）整体移入 `.hawk/trash/`。

#### 请求

| 参数 | 类型   | 必填 | 说明       |
| ---- | ------ | ---- | ---------- |
| path | string | 是   | 文件夹路径 |

#### 响应

```json
{ "status": "success" }
```

### restore

`POST /api/v1/folder/restore`

从回收站恢复文件夹：按原路径放回。原路径已被占用时返回 `FILE_EXISTS`。

#### 请求

| 参数 | 类型   | 必填 | 说明           |
| ---- | ------ | ---- | -------------- |
| path | string | 是   | 原库内相对路径 |

#### 响应

```json
{ "status": "success" }
```

## item

| 方法 | 端点                             | 说明           |
| ---- | -------------------------------- | -------------- |
| POST | `/api/v1/item/list`              | 查询 item 列表 |
| POST | `/api/v1/item/skeleton`          | 全量布局骨架（dim，不分页，与 list 同序） |
| GET  | `/api/v1/item/detail`            | 获取单个 item  |
| GET  | `/api/v1/item/count`             | 获取 item 总数 |
| POST | `/api/v1/item/add`               | 添加新 item    |
| POST | `/api/v1/item/update`            | 更新 item      |
| POST | `/api/v1/item/batch_update`      | 批量更新(标签/分类并集、评分/文件夹设置) |
| POST | `/api/v1/item/delete`            | 移入回收站     |
| POST | `/api/v1/item/restore`           | 从回收站恢复   |
| GET  | `/api/v1/item/thumbnail`         | 获取缩略图     |
| GET  | `/api/v1/item/file`              | 获取原图文件   |
| POST | `/api/v1/item/refresh_thumbnail` | 重新生成缩略图 |
| POST | `/api/v1/item/replace`           | 替换文件内容（客户端编辑后提交存储层） |

### Item 对象

| 字段              | 类型     | 说明                                       |
| ----------------- | -------- | ------------------------------------------ |
| id                | string   | 内容哈希（BLAKE3 hex）                     |
| name              | string   | 文件名（不含扩展名），取主路径             |
| ext               | string   | 扩展名，小写，不含点                       |
| width / height    | number   | 像素尺寸                                   |
| size              | number   | 文件大小（字节）                           |
| url               | string   | 来源网址，可为空                           |
| tags              | string[] | 标签列表                                   |
| categories        | string[] | 分类列表（虚拟分类维度，扁平可多选）       |
| paths             | string[] | 所有文件位置（库内相对路径），首个为主路径 |
| folders           | string[] | 所在文件夹路径列表（由 paths 派生）        |
| star              | number   | 评分 0–5                                   |
| annotation        | string   | 备注                                       |
| modification_time | number   | 修改时间（Unix 毫秒）                      |
| palette           | object[] | 调色板（按占比降序，最多 10 项；未提炼或不可解码时为空数组） |

palette 项：`{ "color": "#344441", "percentage": 3.1 }`——color 为 # 前缀小写 hex，percentage 为像素覆盖占比（0–100，1 位小数）。调色板由后台异步提炼（见 [颜色提炼与颜色检索](color-search.md)），新入库素材稍后才就绪；就绪后通过 `item.updated` 事件推送。

> **同内容去重**：内容相同的文件共享一个 item，`paths` 记录所有文件位置。
>
> **id 漂移**：素材文件内容被修改后，内容哈希变化会导致 id 变化。hawk 将素材视为「入库后基本不变」的数据；重建索引时会按「路径 + 文件名」匹配旧元数据，匹配成功则自动将元数据迁移到新 id。该匹配是启发式的，客户端不应假设 id 永久稳定。

### list

`POST /api/v1/item/list`

过滤条件为复杂结构，故使用 POST。所有过滤参数均为可选，组合逻辑为 AND。

#### 请求

| 参数       | 类型     | 说明                                                            |
| ---------- | -------- | --------------------------------------------------------------- |
| ids        | string[] | 按 id 列表匹配                                                  |
| keywords   | string[] | 关键词（匹配名称、备注）                                        |
| tags       | string[] | 按标签过滤（AND）                                                |
| categories | string[] | 按分类过滤（精确匹配），`categories_match`：`any`（默认）/ `all` |
| exclude_categories | string[] | 排除分类（任一命中即剔除）                    |
| exclude_tags | string[] | 排除标签（任一命中即剔除）                                |
| star       | number   | 按评分过滤                                                      |
| folders    | string[] | 按文件夹路径过滤（含子目录）                                    |
| folders_exact | boolean | 为 true 时文件夹只精确匹配直接位于该目录下的 item（不含子目录）；空字符串表示库根目录，默认 false |
| without_categories | boolean | 只返回未分类（没有任何分类）的 item，默认 false      |
| without_tags | boolean | 只返回未标签（没有任何标签）的 item，默认 false      |
| ext        | string   | 按扩展名过滤                                                    |
| annotation | string   | 按备注文本过滤                                                  |
| url        | string   | 按来源网址过滤                                                  |
| color      | string   | 按颜色检索（`#344441`，`#` 可省略，大小写不敏感）；命中条件为调色板任一颜色 CIE76 ΔE ≤ 25，格式非法返回 `INVALID_PARAM` |
| in_trash   | boolean  | 是否只查回收站中的 item，默认 false                             |
| order_by   | string   | 排序字段：`modification_time`（默认）/ `name` / `size` / `star` |
| order      | string   | 排序方向：`desc`（默认）/ `asc`                                 |
| offset     | number   | 分页偏移，默认 0                                                |
| limit      | number   | 分页大小，默认 50                                               |

#### 响应

```json
{
  "status": "success",
  "data": {
    "items": [
      {
        "id": "9b1f2c...",
        "name": "sunset-photo",
        "ext": "jpg",
        "width": 1920,
        "height": 1080,
        "size": 245760,
        "url": "https://example.com/photo.jpg",
        "tags": ["nature", "sunset"],
        "paths": ["posters/2024/sunset-photo.jpg"],
        "folders": ["posters/2024"],
        "star": 4,
        "annotation": "Beautiful sunset",
        "modification_time": 1700000000000
      }
    ],
    "total": 1250,
    "total_size": 314572800,
    "offset": 0,
    "limit": 50
  }
}
```

`total` / `total_size` 为过滤后未分页的全量计数与字节数合计（前端检查器「分区状态」用）。

**排序稳定性**：主键同值（如相同 `modification_time`）时按 `id` 字典序打破平局，保证相同查询的次序逐位确定——前端「骨架 + 分页窗口」模型依赖这一点对齐。

### skeleton

`POST /api/v1/item/skeleton`

请求参数与 `item/list` 完全相同（`offset` / `limit` 可省略，忽略），过滤、排序与 `item/list` **逐位一致**，但不分页，只返回布局所需的最低字段。供前端虚拟网格一次性建立完整布局（滚动条总高即时确定，可自由拖动跳转），视口内再按 offset 用 `item/list` 取详情。

#### 响应

```json
{
  "status": "success",
  "data": {
    "items": [
      { "id": "9b1f2c...", "width": 1920, "height": 1080, "star": 4 }
    ],
    "total_size": 314572800
  }
}
```

条目数（`items` 长度）即全量计数；`star` 供网格 ★ 角标在未加载详情时显示。

### detail

`GET /api/v1/item/detail?id=<hash>`

返回单个 Item 对象；不存在时返回 `ITEM_NOT_FOUND`。

### count

`GET /api/v1/item/count`

返回库内 item 总数（不含回收站）。

#### 响应

```json
{ "status": "success", "data": 12500 }
```

### add

`POST /api/v1/item/add`

向素材库添加新文件。`path`、`url`、`img_base64` 三者必须提供其一，作为文件内容来源；文件将写入 `folder_path` 指定的真实目录（缺省为库根目录），随后由索引流水线完成哈希与缩略图。`url` 仅作为下载来源；来源网页（图片所在的页面地址）经 `website` 传入并记录为 Item.url。

`path` 导入时保留原文件的创建时间与修改时间（`File.Copy` 默认会重置）：按 `modification_time` 排序与文件管理器观感均以原文件为准；`url`/`img_base64` 无原文件时间，取入库时刻。

#### 请求

| 参数        | 类型     | 必填   | 说明                                   |
| ----------- | -------- | ------ | -------------------------------------- |
| path        | string   | 三选一 | 本地文件路径，导入该文件               |
| url         | string   | 三选一 | 下载该 URL 的文件入库                  |
| img_base64  | string   | 三选一 | Base64 编码的图像数据                  |
| name        | string   | 否     | 文件名（不含扩展名），缺省取来源文件名 |
| folder_path | string   | 否     | 目标文件夹路径，缺省为库根目录         |
| tags        | string[] | 否     | 标签                                   |
| categories  | string[] | 否     | 分类（扁平名称校验；浏览器拖拽收集用）   |
| annotation  | string   | 否     | 备注                                   |
| website     | string   | 否     | 来源网页（Eagle `website` 同义），记录为 Item.url |

#### 响应

Item 对象，并附带 `already_existed` 标志：

- 内容不存在：写入文件并索引，返回新 item，`already_existed: false`
- 内容已存在（同哈希）：仍将文件复制到 `folder_path` 目标位置，已有 item 的 `paths` 追加新路径，返回该 item，`already_existed: true`——客户端可据此提示「内容已存在，已关联到现有条目」

```json
{
  "status": "success",
  "data": {
    "item": {
      "id": "9b1f2c...",
      "paths": ["icons/cat.jpg", "posters/2024/cat.jpg"]
    },
    "already_existed": true
  }
}
```

### update

`POST /api/v1/item/update`

更新元数据（写入 `.hawk/metadata/`）。`name`、`folder_path` 会同步操作真实文件。

#### 请求

| 参数        | 类型     | 必填 | 说明                                               |
| ----------- | -------- | ---- | -------------------------------------------------- |
| id          | string   | 是   | item id                                            |
| path        | string   | 否   | 指定操作的文件位置（同内容多路径时），缺省为主路径 |
| name        | string   | 否   | 重命名文件（同步修改真实文件名）                   |
| tags        | string[] | 否   | 标签（整体替换）                                   |
| categories  | string[] | 否   | 分类（整体替换，自动登记注册表）               |
| folder_path | string   | 否   | 移动到新文件夹（移动真实文件）                     |
| star        | number   | 否   | 评分 0–5                                           |
| annotation  | string   | 否   | 备注                                               |
| url         | string   | 否   | 来源网址                                           |

#### 响应

更新后的 Item 对象。

### batch_update

`POST /api/v1/item/batch_update`

对一批 item 应用同一组更新,一次请求完成(多选打标签/评分/移动等批量场景,避免逐条 `item/update` 的 N 次往返)。所有 id 串行经索引流水线应用,响应一次性返回。

语义与 `item/update` 的差异:

- `add_tags` / `add_categories` 是**并集追加**(保留已有),不是整体替换
- `star` / `folder_path` 是**设置**(与 `item/update` 同语义)

#### 请求

| 参数          | 类型     | 必填 | 说明                                             |
| ------------- | -------- | ---- | ------------------------------------------------ |
| ids           | string[] | 是   | item id 列表(重复 id 自动去重)                 |
| add_tags      | string[] | 否   | 追加标签(并集,自动登记注册表)                  |
| add_categories | string[] | 否  | 追加分类(并集;名称校验同 `item/update`,自动登记注册表) |
| star          | number   | 否   | 评分 0–5(设置)                                 |
| folder_path   | string   | 否   | 移动到该文件夹(移动各 item 的主位置;空字符串为库根目录) |

四个更新字段至少提供一个,否则返回 `INVALID_PARAM`。

#### 部分失败语义

批量操作不整体失败,逐项跳过:

- **内容不存在**的 id:跳过该项(元数据与移动都不应用),记入 `missing_ids`
- **移动冲突**(目标位置已有同名文件):跳过该项的移动,记入 `missing_ids`;`add_tags` / `add_categories` / `star` 照常应用
- **回收站中的 item**(无库内位置):移动不适用,跳过;元数据照常应用

#### 响应

```json
{
  "status": "success",
  "data": { "updated": 498, "missing_ids": ["abc...", "def..."] }
}
```

`updated` 为成功应用元数据的 id 数;`missing_ids` 为上述未达成的 id(去重)。客户端可据此提示「已更新 n 项,m 项未处理」。

每个成功更新的 item 都会照常推送 `item.updated` 事件。

### delete

`POST /api/v1/item/delete`

移入回收站：文件移入 `.hawk/trash/`（保留目录结构），元数据保留。

#### 请求

| 参数 | 类型   | 必填 | 说明                                         |
| ---- | ------ | ---- | -------------------------------------------- |
| id   | string | 是   | item id                                      |
| path | string | 否   | 指定文件位置（同内容多路径时），缺省为主路径 |

#### 响应

```json
{ "status": "success" }
```

### restore

`POST /api/v1/item/restore`

从回收站恢复：文件移回元数据 `paths` 记录的原路径。原路径已被占用时返回 `FILE_EXISTS`。

#### 请求

| 参数 | 类型   | 必填 | 说明                                         |
| ---- | ------ | ---- | -------------------------------------------- |
| id   | string | 是   | item id                                      |
| path | string | 否   | 指定文件位置（同内容多路径时），缺省为主路径 |

#### 响应

```json
{ "status": "success" }
```

### thumbnail

`GET /api/v1/item/thumbnail?id=<hash>&size=256|512|1024`

缩略图为缓存（**扫描导入即时生成**：首扫并行阶段单次解码同出缩略图+调色板+宽高，item 入库即可完整显示；增量/对账不生成，由读取端兜底），响应分三种：

1. **命中缓存** → 缩略图二进制（`image/webp`）
2. **未命中且原图浏览器可渲染**（jpg/png/gif/webp/bmp）→ **直接回源原图**（200，Content-Type 为原图类型），同时后台入队生成缩略图缓存，下次请求即命中 webp
3. **未命中且不可渲染**（tiff 等）→ 404（后台生成中，生成完成后经 `item.updated` 事件重建，前端已有占位重试闭环）

`size` 缺省为 256，可取值来自项目配置的 `thumbnail_sizes`。响应带 `Cache-Control: immutable`——item id 是内容哈希，内容永不变，客户端可永久缓存。注意：情形 2 的原图响应同样带 immutable，客户端可能长期持有原图字节而不升级到 webp（视觉无损）。

### file

`GET /api/v1/item/file?id=<hash>`

返回原图文件二进制，Content-Type 按扩展名推断（无法识别时为 `application/octet-stream`）。桌面端预览浮层用它展示原图（缩略图是压缩过的 WebP）。文件位置取 item 主位置（优先非回收站位置）；文件已缺失时返回 404。与缩略图同理带 `Cache-Control: immutable`。`<img>` 直链无法设置请求头，与 thumbnail 一样放行查询参数 `?token=`。

### refresh_thumbnail

`POST /api/v1/item/refresh_thumbnail`

#### 请求

| 参数 | 类型   | 必填 | 说明    |
| ---- | ------ | ---- | ------- |
| id   | string | 是   | item id |

#### 响应

```json
{ "status": "success" }
```

### replace

`POST /api/v1/item/replace`

替换已有素材的文件内容。面向**客户端编辑**场景（旋转、裁切等）：编辑计算（解码/变换/重编码）在客户端完成，本端点只做存储层的校验、哈希与写盘——server 不承接用户图片的编辑计算，远程部署时同样成立。

内容哈希几乎必然变化 → **id 漂移**：素材参数（标签/评分/备注/分类/来源）按路径继承迁移，事件（`item.removed` 旧 id + `item.added` 新 id）与缩略图/调色板重建由索引流水线闭环（见「Item 对象」的 id 漂移说明）。响应即新 Item 对象（新 id），客户端应切换到新 id 继续引用。

#### 请求

| 参数       | 类型   | 必填 | 说明                                                       |
| ---------- | ------ | ---- | ---------------------------------------------------------- |
| id         | string | 是   | item id                                                    |
| path       | string | 否   | 指定操作的文件位置（同内容多路径时），缺省为主路径         |
| img_base64 | string | 是   | 新内容的 Base64 编码                                       |

约束：

- 目标位置必须在库内，回收站中的 item 先恢复再替换，否则 `INVALID_PARAM`
- 内容必须是可识别的图像，且**格式与文件扩展名一致**（如 `.png` 文件只接受 PNG 内容），否则 `UNSUPPORTED_FORMAT`
- 内容哈希与当前 id 相同时为幂等 no-op，直接返回当前 Item，不触发漂移
- 写回时**保留原文件的修改时间与创建时间**：旋转等修正性编辑不改变素材的时序位置（`modification_time` 排序不受编辑影响）

#### 响应

新 Item 对象（内容未变化时为当前 Item 对象）。

## category

分类是虚拟分类维度：**扁平名字**（无层级），一个 item 可同时挂多个分类。注册表（`.hawk/categories.toml`）支持空分类预创建；item 赋值时自动登记。见 [category.md](category.md)。

| 方法 | 端点 | 说明 |
| ---- | ---- | ---- |
| GET  | `/api/v1/category/list` | `[{ name, count }]`（注册表 ∪ 全部 item 赋值并集），count 为库内（不含回收站）item 数 |
| POST | `/api/v1/category/create` | `{ "name": "海报" }`；已存在返回 `CATEGORY_EXISTS` |
| POST | `/api/v1/category/update` | `{ "name", "new_name" }` 重命名；目标已存在时合并 |
| POST | `/api/v1/category/delete` | `{ "name" }`，全部 item 的相关赋值清除 |

`category/list` 响应与 `tag/list` 同构（`{ name, count }` 数组）。

## tag

标签注册表（`.hawk/tags.toml`）支持空标签预创建；item 赋值时自动登记。

| 方法 | 端点 | 说明 |
| ---- | ---- | ---- |
| GET  | `/api/v1/tag/list` | `[{ "name", "count" }]`，count 为库内（不含回收站）item 数 |
| POST | `/api/v1/tag/create` | `{ "name" }` |
| POST | `/api/v1/tag/update` | `{ "name", "new_name" }`，重命名，全部 item 跟随；目标已存在时合并 |
| POST | `/api/v1/tag/delete` | `{ "name" }`，全部 item 的该标签清除 |

## view

视图偏好（`.hawk/view.toml`，参与同步）：记住文件夹/分类/标签视图各自的排序方式。
条目为扁平 map，scope 键三种形态：

- `folder:<库内路径>`（路径 `""` 为库根）——**继承由客户端解析**：沿父链向上查找，子文件夹自己的设置优先于父级
- `category:<名称>` / `tag:<名称>`——无层级，无条目时回落全局默认（修改时间↓）

服务端只存取原始条目，不理解继承语义。文件夹移动/重命名时 `folder:` 键自动跟随，删除时自动清除。排序值与 `item/list` 的 `order_by`/`order` 同白名单。

| 方法 | 端点 | 说明 |
| ---- | ---- | ---- |
| GET | `/api/v1/view/preferences` | 全部条目：`{ "folder:photos": { "order_by", "order" }, ... }` |
| PUT | `/api/v1/view/preference` | `{ "scope", "order_by", "order" }`，覆盖写；非法 scope/排序值返回 `INVALID_PARAM` |
| DELETE | `/api/v1/view/preference?scope=<scope>` | 删除条目，回到继承/默认 |

## trash

回收站内容通过 `item/list`（`in_trash: true`）查询。

| 方法 | 端点                  | 说明       |
| ---- | --------------------- | ---------- |
| POST | `/api/v1/trash/clear` | 清空回收站 |

### clear

`POST /api/v1/trash/clear`

彻底删除回收站中的全部文件，并清理对应的元数据与缩略图。不可恢复。

#### 响应

```json
{ "status": "success" }
```

## events

`GET /api/v1/events?token=<token>`

Server-Sent Events 订阅素材库变更,前端据此增量刷新界面。`EventSource` 无法设置请求头,token 通过查询参数传递。

### 事件一览

| 事件              | data | 说明 |
| ----------------- | ---- | ---- |
| `item.added`      | Item 对象 | 新文件入库（单条路径：监听/API 增量） |
| `items.added`     | `{ "ids": ["..."] }` | 扫描导入的批量合并事件（300ms 窗口/2000 条上限合成一条）；客户端按「有新增」信号重载列表即可 |
| `item.updated`    | Item 对象 | 元数据、文件位置或调色板变更(缩略图生成完成也补发一次,前端据此重建 404 占位) |
| `items.updated`   | `{ "items": [Item...] }` | `item.updated` 的批量变体（调色板批量回写等），客户端逐个就地替换缓存 |
| `item.trashed`    | `{ "id": "..." }` | 最后一个库内位置移入回收站 |
| `item.restored`   | Item 对象 | 首个回收站位置回归库内 |
| `item.removed`    | `{ "id": "..." }` | 彻底删除(无剩余位置) |
| `folder.changed`  | `{ "reason": "external" }` | 目录结构可能变化,客户端应重拉 `folder/list`;reason 恒为 `external`,客户端必须忽略取值(结构为将来预留) |
| `task.progress`   | `{ "task": "thumbnail", "pending": 236, "active": 4 }` | 后台任务积压变化(缩略图/调色板队列与索引管道;服务端 500ms 节流,积压倒零后补发一帧清零帧) |

事件名与负载即持久契约(Rust 重写必须逐字兼容);后端以常量集中定义(`ItemEvents`),客户端不许凭代码反推。

### 负载契约

**Item 对象**:与 `item/list` 响应中的 Item 结构完全相同(见「Item 对象」节)。`item.updated` / `item.added` / `item.restored` / `items.updated` 带完整对象,客户端可就地替换缓存;`trashView` 由服务端按「是否只剩回收站位置」投影(回收站视图的 `paths` 为原库内路径)。

**id 负载**(`item.trashed` / `item.removed`):

```json
{ "id": "9b1f2c..." }
```

**folder.changed 负载**:

```json
{ "reason": "external" }
```

触发来源:本端文件夹增删改移(API)、外部进程目录操作(文件监听)、周期对账扫描兜底。文件夹树无增量语义(前端经 `folder/list` 全量实时建树),事件只表达「需要重拉」。

**task.progress 负载**:

```json
{ "task": "thumbnail", "pending": 236, "active": 4 }
```

`task` 为 `thumbnail`（缩略图/调色板队列）或 `index`（索引管道：入库排队 job 与写入防抖路径）。`pending` 为排队数，`active` 为执行中（缩略图生成并发度 `CPU/4`、封顶 8；索引任务扫描中为 1、否则为 0）。`task=index` 且扫描进行中时额外携带阶段进度：

```json
{ "task": "index", "pending": 12, "active": 1, "phase": "hash", "processed": 1800, "total": 4200 }
```

`phase` 为 `scan`（遍历，`total=0` 表示总量未知，客户端宜显示不定态）/ `hash` / `apply`；非扫描期间三个字段省略。SSE 断开的客户端可轮询 `GET /api/v1/app/status` 获取同一快照。

### 时序与可靠性语义

- **节流**:`task.progress` 服务端 500ms 最多一帧;`item.*` 事件无节流,批量操作(如 `item/batch_update` 500 项)会连续收到多条
- **不保证送达**:订阅者消费跟不上(积压 1024 条)时服务端直接断开该订阅——客户端重连后必须以 `item/skeleton` + `folder/list` 全量对齐,不得假设收到过全部事件
- **初始索引期间**:就绪网关拦截期内不推事件(订阅端点同样 503),主界面加载完成后订阅即可
- **顺序**:同一 item 的事件按流水线处理顺序发出;不同 item 之间无全局顺序保证
- **重连**:EventSource 断线自动重连,`onopen` 再次触发即对齐时机(参考前端 `events.ts` 的 `onReconnect` 约定)

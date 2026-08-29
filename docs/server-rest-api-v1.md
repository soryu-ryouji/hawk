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

错误码：`INVALID_PARAM`、`ITEM_NOT_FOUND`、`FOLDER_NOT_FOUND`、`FILE_EXISTS`、`UNSUPPORTED_FORMAT`、`CATEGORY_NOT_FOUND`、`CATEGORY_EXISTS`、`TAG_NOT_FOUND`、`INTERNAL`

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
| GET  | `/api/v1/app/status` | 后台任务积压（缩略图队列；SSE 的 `task.progress` 事件为同一快照的推送版） |
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
| phase | string | 仅 starting 时存在：`scan`（遍历清单，total 恒 0）/ `hash`（并行算哈希）/ `apply`（应用索引） |
| processed / total | number | 仅 starting 时存在；`total=0` 表示该阶段总量未知（客户端宜显示不定态进度） |
| message | string | 仅 error 时存在：失败原因 |

### status

`GET /api/v1/app/status`

后台任务积压快照。当前仅缩略图（含调色板）worker 队列：导入或初次索引后大量素材等待生成缩略图时，`pending`（排队中）与 `active`（生成中，上限 4）大于 0；积压清空后两者归零。

SSE 客户端建议直接订阅 `task.progress` 事件（同一快照的推送版，服务端 500ms 节流）；本端点供轮询型客户端使用。

#### 响应

```json
{
  "status": "success",
  "data": { "thumbnail": { "pending": 236, "active": 4 } }
}
```

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
    "exec_path": "C:/Tools/hawk/hawk-server.exe"
  }
}
```

| 字段      | 类型   | 说明                          |
| --------- | ------ | ----------------------------- |
| version   | string | 后端版本号                    |
| platform  | string | `windows` / `macos` / `linux` |
| exec_path | string | 后端可执行文件路径            |

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
| POST | `/api/v1/item/delete`            | 移入回收站     |
| POST | `/api/v1/item/restore`           | 从回收站恢复   |
| GET  | `/api/v1/item/thumbnail`         | 获取缩略图     |
| GET  | `/api/v1/item/file`              | 获取原图文件   |
| POST | `/api/v1/item/refresh_thumbnail` | 重新生成缩略图 |

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

返回缩略图二进制（`image/webp`）。`size` 缺省为 256，可取值来自项目配置的 `thumbnail_sizes`。响应带 `Cache-Control: immutable`——item id 是内容哈希，缩略图内容永不变，客户端可永久缓存。缩略图不存在时返回 404（首次索引完成前可能出现）。

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

Server-Sent Events 订阅素材库变更，前端据此增量刷新界面。`EventSource` 无法设置请求头，token 通过查询参数传递。

事件类型：

| 事件            | data              | 说明             |
| --------------- | ----------------- | ---------------- |
| `item.added`    | Item 对象         | 新文件入库       |
| `item.updated`  | Item 对象         | 元数据或文件变更 |
| `item.trashed`  | `{ "id": "..." }` | 移入回收站       |
| `item.restored` | Item 对象         | 从回收站恢复     |
| `item.removed`  | `{ "id": "..." }` | 彻底删除         |
| `task.progress` | `{ "task": "thumbnail", "pending": 236, "active": 4 }` | 后台任务积压变化（当前仅缩略图队列；服务端 500ms 节流，积压倒零后补发一帧清零帧） |

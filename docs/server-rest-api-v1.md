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

错误码：`INVALID_PARAM`、`ITEM_NOT_FOUND`、`FOLDER_NOT_FOUND`、`FILE_EXISTS`、`UNSUPPORTED_FORMAT`、`INTERNAL`

### ID 规范

- **item id**：文件内容的 BLAKE3 哈希（hex），与存储设计一致
- **library id**：UUID
- **folder**：不使用合成 id，直接以相对素材库根目录的真实目录路径标识（如 `posters/2024`）

### 其他

- 时间戳均为 Unix 毫秒
- 分页参数：`offset`（默认 0）、`limit`（默认 50）
- 桌面版所有请求需携带启动时下发的 token（`Authorization: Bearer <token>`）；SSE 无法设置请求头，改用查询参数 `?token=`

## app

| 方法 | 端点               | 说明         |
| ---- | ------------------ | ------------ |
| GET  | `/api/v1/app/info` | 获取应用信息 |

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

| 字段     | 类型   | 说明                          |
| -------- | ------ | ----------------------------- |
| version  | string | 后端版本号                    |
| platform | string | `windows` / `macos` / `linux` |
| exec_path | string | 后端可执行文件路径            |

### health

`GET /health`

就绪探活，不带 `/api/v1` 前缀，无需 token。Electron 壳启动后轮询此端点，返回 200 即表示后端就绪。

## library

hawk-server 单实例对应单个素材库。

| 方法 | 端点                   | 说明               |
| ---- | ---------------------- | ------------------ |
| GET  | `/api/v1/library/info` | 获取当前素材库信息 |

### info

`GET /api/v1/library/info`

获取当前打开的素材库信息与标签组。文件夹树通过 `folder/list` 获取。

#### 响应

```json
{
  "status": "success",
  "data": {
    "id": "32455218-9e79-61ca-7e1d-034c0ed9f33b",
    "name": "设计素材库",
    "path": "D:/Assets/Design",
    "tags_groups": [
      {
        "id": "c549d2a8-c187-c612-617f-83fcef4976a2",
        "name": "Location",
        "tags": ["Kitchen"],
        "color": "yellow"
      }
    ],
    "modification_time": 1592461625783,
    "application_version": "1.0.0"
  }
}
```

## folder

folder 即素材库中的真实目录。对 folder 的操作会直接操作文件系统，由文件监听同步到索引。

| 方法 | 端点                    | 说明         |
| ---- | ----------------------- | ------------ |
| GET  | `/api/v1/folder/list`   | 列出文件夹树 |
| POST | `/api/v1/folder/create` | 创建文件夹   |
| POST | `/api/v1/folder/update` | 更新文件夹   |
| POST | `/api/v1/folder/delete` | 删除文件夹   |

### list

`GET /api/v1/folder/list`

返回完整文件夹树。节点字段：`path`（相对库根目录）、`name`、`children`、`modification_time`。

### create

`POST /api/v1/folder/create`

#### 请求

| 参数        | 类型   | 必填 | 说明                                           |
| ----------- | ------ | ---- | ---------------------------------------------- |
| name        | string | 是   | 文件夹名称                                     |
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

## item

| 方法 | 端点                             | 说明                |
| ---- | -------------------------------- | ------------------- |
| POST | `/api/v1/item/list`              | 查询 item 列表      |
| GET  | `/api/v1/item/detail`            | 获取单个 item       |
| GET  | `/api/v1/item/count`             | 获取 item 总数      |
| POST | `/api/v1/item/add`               | 添加新 item         |
| POST | `/api/v1/item/update`            | 更新 item           |
| GET  | `/api/v1/item/thumbnail`         | 获取缩略图          |
| POST | `/api/v1/item/refresh_thumbnail` | 重新生成缩略图      |

### Item 对象

| 字段             | 类型     | 说明                   |
| ---------------- | -------- | ---------------------- |
| id               | string   | 内容哈希（BLAKE3 hex） |
| name             | string   | 文件名（不含扩展名）   |
| ext              | string   | 扩展名，小写，不含点   |
| width / height   | number   | 像素尺寸               |
| size             | number   | 文件大小（字节）       |
| url              | string   | 来源网址，可为空       |
| tags             | string[] | 标签列表               |
| folders          | string[] | 所在文件夹路径列表   |
| star             | number   | 评分 0–5               |
| annotation       | string   | 备注                   |
| is_deleted       | boolean  | 是否在回收站           |
| modification_time | number  | 修改时间（Unix 毫秒）  |

> **id 漂移**：素材文件内容被修改后，内容哈希变化会导致 id 变化。hawk 将素材视为「入库后基本不变」的数据；重建索引时会按「路径 + 文件名」匹配旧元数据，匹配成功则自动将元数据迁移到新 id。该匹配是启发式的，客户端不应假设 id 永久稳定。

### list

`POST /api/v1/item/list`

过滤条件为复杂结构，故使用 POST。所有过滤参数均为可选，组合逻辑为 AND。

#### 请求

| 参数       | 类型     | 说明                       |
| ---------- | -------- | -------------------------- |
| ids        | string[] | 按 id 列表匹配             |
| keywords   | string[] | 关键词（匹配名称、备注）   |
| tags       | string[] | 按标签过滤                 |
| folders    | string[] | 按文件夹路径过滤           |
| ext        | string   | 按扩展名过滤               |
| annotation | string   | 按备注文本过滤             |
| url        | string   | 按来源网址过滤             |
| is_deleted | boolean  | 是否只查回收站，默认 false |
| order_by   | string   | 排序字段：`modification_time`（默认）/ `name` / `size` / `star` |
| order      | string   | 排序方向：`desc`（默认）/ `asc` |
| offset     | number   | 分页偏移，默认 0           |
| limit      | number   | 分页大小，默认 50          |

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
        "folders": ["posters/2024"],
        "star": 4,
        "annotation": "Beautiful sunset",
        "is_deleted": false,
        "modification_time": 1700000000000
      }
    ],
    "total": 1250,
    "offset": 0,
    "limit": 50
  }
}
```

### detail

`GET /api/v1/item/detail?id=<hash>`

返回单个 Item 对象；不存在时返回 `ITEM_NOT_FOUND`。

### count

`GET /api/v1/item/count`

#### 响应

```json
{ "status": "success", "data": 12500 }
```

### add

`POST /api/v1/item/add`

向素材库添加新文件。`path`、`url`、`img_base64` 三者必须提供其一，作为文件内容来源；文件将写入 `folder_path` 指定的真实目录（缺省为库根目录），随后由索引流水线完成哈希与缩略图。以 `url` 下载时，该 URL 自动记录为 Item.url。

#### 请求

| 参数       | 类型     | 必填   | 说明                                   |
| ---------- | -------- | ------ | -------------------------------------- |
| path       | string   | 三选一 | 本地文件路径，导入该文件               |
| url        | string   | 三选一 | 下载该 URL 的文件入库                  |
| img_base64 | string   | 三选一 | Base64 编码的图像数据                  |
| name       | string   | 否     | 文件名（不含扩展名），缺省取来源文件名 |
| folder_path | string   | 否     | 目标文件夹路径，缺省为库根目录         |
| tags       | string[] | 否     | 标签                                   |
| annotation | string   | 否     | 备注                                   |

#### 响应

Item 对象。`id` 在索引完成后生成，若内容已存在则返回已有 item。

### update

`POST /api/v1/item/update`

更新元数据（写入 `.hawk/metadata/`）。`name`、`folder_path`、`is_deleted` 会同步操作真实文件。

#### 请求

| 参数       | 类型     | 必填 | 说明                             |
| ---------- | -------- | ---- | -------------------------------- |
| id         | string   | 是   | item id                          |
| name       | string   | 否   | 重命名文件（同步修改真实文件名） |
| tags       | string[] | 否   | 标签（整体替换）                 |
| folder_path | string   | 否   | 移动到新文件夹（移动真实文件）   |
| star       | number   | 否   | 评分 0–5                         |
| annotation | string   | 否   | 备注                             |
| url        | string   | 否   | 来源网址                         |
| is_deleted | boolean  | 否   | 移入/移出回收站（`.hawk/trash/`）  |

#### 响应

更新后的 Item 对象。

### thumbnail

`GET /api/v1/item/thumbnail?id=<hash>&size=256|1024`

返回缩略图二进制（`image/webp`）。`size` 缺省为 256，可取值来自项目配置的 `thumbnail_sizes`。响应带 `Cache-Control: immutable`——item id 是内容哈希，缩略图内容永不变，客户端可永久缓存。缩略图不存在时返回 404（首次索引完成前可能出现）。

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

## events

`GET /api/v1/events?token=<token>`

Server-Sent Events 订阅素材库变更，前端据此增量刷新界面。`EventSource` 无法设置请求头，token 通过查询参数传递。

事件类型：

| 事件           | data              | 说明             |
| -------------- | ----------------- | ---------------- |
| `item.added`   | Item 对象         | 新文件入库       |
| `item.updated` | Item 对象         | 元数据或文件变更 |
| `item.deleted` | `{ "id": "..." }` | 文件删除         |

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

错误码：`INVALID_PARAM`、`ITEM_NOT_FOUND`、`FOLDER_NOT_FOUND`、`FILE_EXISTS`、`UNSUPPORTED_FORMAT`、`CATEGORY_NOT_FOUND`、`CATEGORY_EXISTS`、`TAG_NOT_FOUND`、`INTERNAL`、`READ_ONLY`（只读 viewer/远程写令牌访问未授权写端点，403；见 storage.md 的 `[web]` 与远程 `writable` 写权限配置）、`UNAUTHORIZED`（token 缺失或无效，401）、`NOT_READY`（初始索引构建中，503；唯一放行端点为 `app/startup`）

### ID 规范

- **item id**：文件内容的 BLAKE3 哈希（hex），与存储设计一致
- **library**：不使用合成 id，以素材库根目录路径区分；显示名取 `config.toml` 的 `name`，缺省为库目录名
- **folder**：不使用合成 id，直接以相对素材库根目录的真实目录路径标识（如 `posters/2024`）

### 其他

- 时间戳均为 Unix 毫秒
- 分页参数：`offset`（默认 0）、`limit`（默认 50）
- 客户端携带 `Accept-Encoding: gzip` 时，JSON/文本响应使用 gzip 压缩；图片与 SSE 不压缩。远程隧道与网页版代理原样转发 `Content-Encoding`
- 桌面版默认监听 `27371` 端口（用户可在设置面板「网络 → 本地服务」配置；端口被占用时**不会**静默换端口——daemon 以退出码 4 + `PORT_IN_USE <port>` 退出，app 在启动屏询问换端口），所有请求需携带启动时下发的 token（`Authorization: Bearer <token>`）；SSE 与 `<img>` 直链（thumbnail、file）无法设置请求头，改用查询参数 `?token=`

## app

| 方法 | 端点               | 说明         |
| ---- | ------------------ | ------------ |
| GET  | `/api/v1/app/info` | 获取应用信息 |
| GET  | `/api/v1/app/startup` | 启动状态与索引构建进度（就绪网关唯一放行端点） |
| GET  | `/api/v1/app/status` | 后台任务积压（缩略图队列 + 索引管道；SSE 的 `task.progress` 事件为同一快照的推送版） |
| POST | `/api/v1/app/session` | LAN Web：AccessKey 换有期 Session，以 HttpOnly/Secure Cookie 下发 |
| GET  | `/api/v1/app/admin-token` | 发现本机 LocalIntegration Session（免鉴权，仅限扩展类客户端） |
| GET  | `/api/v1/app/connection` | 当前连接描述符（本地库 / 正在浏览的远端库），生态客户端跟随目标用（admin 限定） |
| GET  | `/api/v1/app/lan` | 读取局域网网站与 hawk-app 直连配置、本机身份及运行状态（admin 限定） |
| PUT  | `/api/v1/app/lan` | 写回 `[web]` 配置并热重绑局域网监听（admin 限定，失败自动回滚） |
| GET  | `/api/v1/app/lan/peers` | 探测同网段可配对的 hawk-app（admin 限定） |
| GET  | `/api/v1/app/lan/clients` | 目标端查看当前活跃的 hawk-app 直连设备（admin 限定） |
| POST | /api/v1/app/lan/device-keys | 创建一把设备访问密钥（admin；不触碰网站密钥/开关） |
| PATCH | /api/v1/app/lan/device-keys/{key_id} | 调整设备密钥读写权限（admin） |
| DELETE | /api/v1/app/lan/device-keys/{key_id} | 删除设备密钥并撤销共享（admin） |
| POST | `/api/v1/app/lan/pair-requests` | 源端发起双向配对请求（免 Bearer，仅直连端口） |
| GET  | `/api/v1/app/lan/pair-requests` | 目标端待确认配对请求列表（admin 限定） |
| GET  | `/api/v1/app/lan/pair-requests/{request_id}` | 源端轮询配对结果（免 Bearer，request_id 为随机凭证） |
| POST | `/api/v1/app/lan/pair-requests/{request_id}/decision` | 目标端批准/拒绝并返回反向设备 profile（admin 限定） |

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
    "index": { "pending": 12, "active": 0, "phase": "hash", "processed": 1800, "total": 4200 },
    "last_scan": { "started_unix_ms": 1788874284637, "duration_ms": 133, "files": 0, "dirty_dirs": 0, "applied": 0 },
    "queue_overflow": false,
    "sse_lagged": 0
  }
}
```

`index.phase` / `processed` / `total` 仅扫描进行中存在：`total=0` 的遍历阶段表示总量未知（客户端宜显示不定态进度）；空闲时为 `null`。

观测字段（均可缺省，不影响客户端）：

| 字段 | 说明 |
| ---- | ---- |
| `last_scan` | 最近一轮全库扫描统计（未跑过为 null）：开始时间、耗时、枚举文件数、深入目录数、实际应用数 |
| `queue_overflow` | 索引队列曾溢出（已触发兜底扫描）；下一轮扫描收尾后复位 |
| `config_error` | `.hawk/config.toml` 解析错误（**保留上次有效配置继续运行**，修复后自动清除）；正常不出现 |
| `sse_lagged` | SSE 订阅因消费落后被断开（lagged）的累计次数 |

### info

`GET /api/v1/app/info`

获取当前 hawk-daemon 的运行信息，可用于判断客户端环境能力。

#### 响应

```json
{
  "status": "success",
  "data": {
    "version": "1.0.0",
    "platform": "windows",
    "exec_path": "C:/Tools/hawk/hawk-daemon.exe",
    "role": "admin",
    "channel": "loopback",
    "has_library": true,
    "lan": { "active": true, "port": 27372, "peer_active": true, "peer_port": 27375 }
  }
}
```

| 字段      | 类型   | 说明                          |
| --------- | ------ | ----------------------------- |
| version   | string | 后端版本号                    |
| platform  | string | `windows` / `macos` / `linux` |
| exec_path | string | 后端可执行文件路径            |
| role | string | 当前凭证角色：`viewer` / `importer` / `editor` / `admin` |
| channel | string | 当前请求通道：`loopback` / `lan_web` / `lan_peer` / `wan`；前端据此判断远程低流量策略 |
| has_library | boolean | 当前 daemon 是否打开本地素材库；false 为无库远程查看形态 |
| lan       | object | 局域网监听实况：`active`（是否在监听）、`port`（active 时存在）、`error`（绑定失败原因，如端口被占用）。配置读写走 `app/lan`（PUT 内置收敛等待），此字段供状态展示 |

### connection

`GET /api/v1/app/connection`（admin 限定）

当前连接描述符：本地 hawk 现在在看哪个素材库。浏览器插件用它把「跟随本地 hawk」落到具体目标——本地库时返回 `local`（调用方继续用自己的环回地址 + admin token），正在浏览远端库时返回会话代理地址与会话令牌，插件直接打该代理即可把素材写进远端库；无本地库且无远端会话时返回 `none`。远程查看模式（无库 daemon）下同样可用。

连接角色由 `role` 表示：本地库为 admin；远端库为 viewer 或 importer，取决于目标设备的 allow_import 与会话授权（目标 daemon 每请求二次执法）。

#### 响应

```json
{
  "status": "success",
  "data": {
    "mode": "remote",
    "api_base": "http://127.0.0.1:57123",
    "credential": "会话临时凭证",
    "device_id": "d_aaba43017fff4888",
    "label": "台式机 · 我的素材库",
    "writable": true
  }
}
```

| 字段     | 类型    | 说明                                                         |
| -------- | ------- | ------------------------------------------------------------ |
| mode     | string  | `local`（本地库）/ `remote`（正在浏览的远端库）/ `none`（未连接任何素材库） |
| api_base | string? | 仅 remote：会话代理环回地址                                  |
| credential | string? | 仅 remote：会话临时凭证（与 app 渲染进程同一把）             |
| device_id| string? | 仅 remote：目标设备 id                                       |
| label    | string? | 仅 remote：目标展示名（设备名 · 库名）                       |
| role | string | 当前连接角色：viewer / importer / editor / admin                                             |

### health

`GET /health`

就绪探活，不带 `/api/v1` 前缀，无需 token。**初始索引完成前返回 503**，完成后 200。只用于区分进程状态，进度与就绪判断以 `app/startup` 为准。

### admin-token

`GET /api/v1/app/admin-token`

免鉴权的 token 发现端点，供浏览器插件等生态客户端零配置接入（插件端实现见 hawk-browser-extension）。

安全性约束：

- 响应**不携带 CORS 头**（`DisableCors`）：跨源网页 JS 无法读取响应，只有持 `host_permissions` 的浏览器扩展能读
- **Host 必须是环回地址**（`127.0.0.1` / `localhost` / `::1`），否则返回 `INVALID_HOST`：防 DNS rebinding 伪装同源读取

拿到 token 后，其余 `/api/*` 请求照常 `Authorization: Bearer <token>`。

#### 响应

```json
{ "status": "success", "data": "<random-token>" }
```

### lan

`GET /api/v1/app/lan`

读取局域网网站访问配置（`.hawk/config.toml` 的 `[web]` 段）与监听运行状态。**仅 admin 可用**：viewer 返回 403 `READ_ONLY`——响应含每把访问密钥的完整 token，只读 viewer 拿到可写密钥即提权。

#### 响应

```json
{
  "status": "success",
  "data": {
    "enabled": true,
    "port": 27372,
    "peer_port": 27375,
    "peer_id": "9f2c1a7b4e8d0356",
    "name": "studio-mac",
    "library_name": "我的素材库",
    "keys": [
      { "id": "key_a1b2", "label": "我的手机", "access_key": "<access-key>", "role": "viewer", "created_at_ms": 1760000000000 }
    ],
    "peer_keys": [
      { "id": "peer_c3d4", "label": "另一台 hawk-app", "access_key": "<peer-key>", "role": "viewer", "created_at_ms": 1760000000000 }
    ],
    "active": true,
    "peer_active": true,
    "peer_error": null
  }
}
```

`active` / `peer_active` 为两个监听器的实况；`error` / `peer_error` 在绑定失败时携带原因。`peer_id`/`name`/`library_name` 是本机设备身份，用于源端生成反向配对 profile。

---

`PUT /api/v1/app/lan`

写回 `[web]` 配置并热重绑局域网监听：daemon 权威写配置（toml_edit 就地改写 `[web] enabled/port`、`[[web.keys]]` 与 `[[web.peer_keys]]`，原子写）→ 唤醒 supervisor 重绑 → 等待本轮收敛后返回。绑定失败（端口占用等）**自动回滚旧配置**并返回错误，不重启进程、不断 SSE。仅 admin 可用。

#### 请求

```json
{
  "enabled": true,
  "port": 27372,
  "peer_port": 27375,
  "keys": [
    { "id": "key_a1b2", "label": "我的手机", "access_key": "<access-key>", "role": "viewer", "created_at_ms": 1760000000000 }
  ],
  "peer_keys": [
    { "id": "peer_c3d4", "label": "另一台 hawk-app", "access_key": "<peer-key>", "role": "viewer", "created_at_ms": 1760000000000 }
  ]
}
```

校验（`INVALID_PARAM`）：`port` 与 `peer_port` 均须为 1–65535 且不能相同；`enabled=true` 时至少一把网站访问密钥；同类密钥最多 32 把，id/access_key 全局非空且不重复。网站端口只接受 `keys` 并托管网页；直连端口只接受 `peer_keys` 且不提供静态页面，网站关闭时仍独立监听。若目标端口被其它进程占用，daemon 会从该端口向后自动选择最近的可用端口并在响应中返回实际值，避免用户手动排障。密钥 `role=editor` 才允许上传/删除/修改等写操作，`viewer` 只读。响应同 GET（收敛后的最新状态）。

### 局域网互联（不经 hawk-server）

daemon 常驻 UDP 发现响应器（`0.0.0.0:27374`，组播 `239.255.72.75` + 广播），只要 hawk-app 直连端口就绪就应答，不依赖网站开关或已存在的 peer key。公告只含设备名、库名、端口与写能力提示，**不含任何 token**。

桌面配对协议为 v3，支持双向与无本地库查看方的单向授权：

- `POST /api/v1/app/lan/pair-requests`：源端创建请求；有本地库时携带 `reverse`（本机 peer_id/name/library_name/候选 urls/peer_key/role），无本地库查看方省略；请求 id 随机、90 秒过期。
- `GET /api/v1/app/lan/pair-requests`：目标端本机 admin 查看待确认列表。
- `GET /api/v1/app/lan/pair-requests/{request_id}`：源端轮询结果，响应含 `protocol=3`、`status` 与批准后的目标 token；旧协议客户端会被拒绝。
- `POST /api/v1/app/lan/pair-requests/{request_id}/decision`：目标端批准/拒绝并选择 A 的读写权限；批准时动态创建该设备专用访问密钥。双向请求在响应 `peer` 中返回源端回连 profile，单向请求返回 `peer=null`。
- `GET /api/v1/app/lan/clients`：目标端本机 admin 查看当前活跃直连设备（key_id、label、last_seen_ms、online、write，以及配对密钥持久化的 peer_id/library_name/url）；鉴权成功后按 peer key 记录最近活跃时间，30 秒窗口内视为在线。
- POST /api/v1/app/lan/device-keys / PATCH / DELETE：设备访问密钥的独立管理面，只操作 [web].peer_keys，不要求存在网站访问密钥，也不会改变网站开关与 [web].keys。配对源端创建同名反向密钥、目标端审批创建访问密钥、设置面板切换权限/撤销设备均走这里。桌面配对创建的设备密钥会持久化可选的 `peer_id` / `library_name` / `url` 元数据，使服务提供方即使尚未加载对方回连 profile，也能显示对方计算机名、库名与直连地址。

LAN 设备密钥与 `/remote/keys` 的创建、权限变更、删除均通过 SSE `lan.access.changed` 通知客户端；客户端重新读取 `app/info` 及两处设置页的 AccessKey 缓存。旧的一次性配对码端点已移除。

## remote

远程访问（接入自托管 hawk-server，见 docs/backend/remote-access.md）。**仅 admin 可用**：viewer（含可写）返回 403 `READ_ONLY`——响应含接入密钥与设备身份信息。

| 方法 | 端点 | 说明 |
| ---- | ---- | ---- |
| GET | `/api/v1/remote` | 读取远程访问接入配置（应用级 `remote.toml`）与信令运行状态（admin 限定） |
| PUT | `/api/v1/remote` | 写回应用级 `remote.toml` 并热重启信令（admin 限定） |
| GET | `/api/v1/remote/devices` | 拉取服务端设备列表（admin 限定） |
| DELETE | `/api/v1/remote/devices/{device_id}` | 注销设备记录（admin 限定；转发服务端，会话与 key 映射一并清理；本机当前设备不可移除） |
| PATCH | `/api/v1/remote/device` | 更新本机设备信息（显示名/allow_remote/库名，admin 限定；allow_import 由目标设备配置上报，不经此端点修改） |
| POST | `/api/v1/remote/device/rotate` | 证书轮换（重新生成自签证书并同步服务端，admin 限定） |
| GET | `/api/v1/remote/session` | 读取当前活动远程会话（admin 限定，无会话 data 为 null） |
| POST | `/api/v1/remote/session` | 建立到目标设备的远程会话并拉起会话代理（admin 限定） |
| DELETE | `/api/v1/remote/session` | 断开当前远程会话（admin 限定，幂等） |
| POST | `/api/v1/remote/access-connect` | AccessKey 原生连接：`lan_url` 存在时走目标 LAN rendezvous，否则走 hawk-server；target_identity 钉扎 + 隧道内登录 |
| POST | `/api/v1/remote/access-session` | AccessKey rendezvous 的隧道内登录（未认证隧道仅此端点可用） |
| POST | `/api/v1/remote/lease` | 已认证隧道续期，保持原角色并延长目标租约 |
| POST | `/api/v1/rendezvous` | 原生 LAN AccessKey rendezvous，仅编排 iroh，不传输素材凭证 |
| GET | `/api/v1/rendezvous/{session_id}` | 查询 rendezvous 状态与目标连接信息 |
| DELETE | `/api/v1/rendezvous/{session_id}` | 取消 rendezvous 并关闭对应隧道 |
| GET | `/api/v1/remote/keys` | 列出本机签发的访问密钥（hawk-app key，admin 限定；不含明文） |
| POST | `/api/v1/remote/keys` | 签发访问密钥（admin 限定；明文只在响应中出现一次） |
| PATCH | `/api/v1/remote/keys/{key_id}` | 更新访问密钥标签/权限（admin 限定） |
| DELETE | `/api/v1/remote/keys/{key_id}` | 吊销访问密钥（admin 限定） |

### remote

`GET /api/v1/remote`

读取应用级 `remote.toml`（`~/.config/hawk/remote.toml`；独立运行 daemon 时回退 `.hawk/remote.toml`）与 remote supervisor 的运行快照。配置跨素材库共享，切换库不丢失。

#### 响应

```json
{
  "status": "success",
  "data": {
    "enabled": true,
    "url": "https://hawk.example.com",
    "tenant_key": "hs_<tenant-key>",
    "allow_import": false,
    "status": {
      "state": "online",
      "server_url": "https://hawk.example.com",
      "device_id": "d_42"
    }
  }
}
```

`status.state`：`disabled`（未启用/配置不完整）、`connecting`（注册/重连中）、`online`（控制通道已连接）、`error`（致命错误，如 tenant-key 无效，`error` 字段携带服务端返回的原因）。

`allow_import` 对应应用级远程配置的 `allow_import`：允许广域网写会话向当前库导入素材/新建文件夹，与 `[web]` 的局域网写权限独立；变更热生效，不重启信令。

---

`PUT /api/v1/remote`

写回应用级 `remote.toml` 并热重启信令：daemon 权威写配置（同 `app/lan` 模型）→ 唤醒 remote supervisor，连接参数变化才重启信令。接入是异步过程（注册/心跳/WSS 重连由 supervisor 驱动），进展经 GET 轮询。

#### 请求

```json
{ "enabled": true, "url": "https://hawk.example.com", "tenant_key": "hs_<tenant-key>", "allow_import": false }
```

校验（`INVALID_PARAM`）：`enabled` 时 `url`/`tenant_key` 必填；`url` 须以 `http://` 或 `https://` 开头。`allow_import` 省略按 false。响应同 GET。

---

`GET /api/v1/remote/devices`

转发拉取 hawk-server 的设备列表（本机与对端同列），接入 key 不出本进程。未配置接入返回 400 `INVALID_PARAM`；服务端不可达/被拒返回 502 `REMOTE_UPSTREAM`。

#### 响应

```json
{
  "status": "success",
  "data": {
    "devices": [
      {
        "device_id": "d_42",
        "name": "家里台式机",
        "platform": "windows",
        "library_name": "我的素材库",
        "online": true,
        "allow_remote": true,
        "allow_import": false,
        "last_seen_at_ms": 1750000000000
      }
    ]
  }
}
```

---

`POST /api/v1/remote/session`

建立远程会话：发起 → 轮询 ready → 候选依序连接（单候选 5s）→ 拉起会话代理（QUIC 指纹钉扎端到端）。单活动会话模型：新会话自动顶替旧会话。会话状态错误（对方拒绝/不可达/超时/直连失败）返回 409 `REMOTE_SESSION_FAILED`，服务端不可达返回 502 `REMOTE_UPSTREAM`。

#### 请求与响应

```json
// 请求（mode 缺省 direct；直连失败且用户确认后以 relay 另建，经服务端中继、带宽自担）
{ "target_device_id": "d_42", "mode": "direct" }
// 响应：app 连接描述符的原料（api_base = http://127.0.0.1:<proxy_port>）
{
  "status": "success",
  "data": {
    "session_id": "s_9",
    "target_device_id": "d_42",
    "target_name": "家里台式机",
    "target_library_name": "我的素材库",
    "proxy_port": 51234,
    "session_token": "<会话代理 token>"
  }
}
```

---

`GET /api/v1/remote/session`

读取当前活动会话（结构同 POST 响应的 data）；无活动会话时 data 为 null。

---

`DELETE /api/v1/remote/session`

断开当前会话（幂等）：通知服务端取消、断开隧道、关闭会话代理。返回 `{"status":"success"}`。

---

`PATCH /api/v1/remote/device`

更新本机设备在服务端的信息（转发服务端 `PATCH /devices/{self}`）：`{ "name": "新名字", "allow_remote": false, "library_name": "工作库" }`，至少提供一项。响应为更新后的设备对象（同 `remote/devices` 单条结构）。

---

`POST /api/v1/remote/device/rotate`

证书轮换：重新生成 QUIC 自签证书（device_id 不变），先同步服务端新指纹再落盘并热重启信令；旧证书的会话随即失效（TOFU 锚点更新）。响应 `{ "fingerprint": "<新指纹>" }`。

---

`GET /api/v1/remote/keys`

列出本机签发的访问密钥（ hawk-app key，供网页版/插件等外部查看者使用）。**不含明文**：明文只在签发响应中出现一次，本地只存 sha256 验证子与前缀预览。

#### 响应

```json
{
  "status": "success",
  "data": {
    "keys": [
      {
        "id": "k_8f3a…",
        "label": "手机插件",
        "read": true,
        "write": true,
        "prefix": "hk_a1",
        "created_at_ms": 1750000000000
      }
    ]
  }
}
```

---

`POST /api/v1/remote/keys`

签发访问密钥。`role` 为 viewer/importer；`custom` 省略时程序生成 `hk_` 前缀的 128 位随机串，自定义时纯读至少 4 字符、携带写权限至少 12 字符（`INVALID_PARAM`）。签发后向 server 全量推送本设备的 key 映射（`PUT /devices/{id}/keys`，best-effort）。

#### 请求与响应

```json
// 请求
{ "label": "手机插件", "role": "importer", "custom": null }
// 响应：key 明文只在本次出现；synced=false 表示远程未接入/推送失败（下次接入自动重试）
{ "status": "success", "data": { "id": "k_8f3a…", "key": "hk_a1b2…", "synced": true } }
```

---

`PATCH /api/v1/remote/keys/{key_id}`

更新标签/权限：`{ "label": "新名", "role": "viewer" }`。权限收窄对后续会话立即生效。响应 `{ "key": <条目>, "synced": true }`。

---

`DELETE /api/v1/remote/keys/{key_id}`

吊销：本地删除并推送 server 移除映射，该 key 的后续会话请求被本机拒绝。响应 `{ "synced": true }`。

## library

hawk-daemon 单实例对应单个素材库。

| 方法 | 端点                      | 说明               |
| ---- | ------------------------- | ------------------ |
| GET  | `/api/v1/library/info`    | 获取当前素材库信息 |
| PATCH | `/api/v1/library/info`   | 改库显示名（`{"name": "..."}`，写 `.hawk/config.toml`，广播 `library.updated`） |
| PUT | `/api/v1/library/scan`   | 周期兜底重扫设置（`{"periodic": bool, "interval": 秒}`，写 `.hawk/config.toml` 的 `[scan]`，保存即热生效） |
| POST | `/api/v1/library/reindex` | 全量重建索引       |
| POST | `/api/v1/library/rescan`  | 强制重新遍历文件系统 |
| POST | `/api/v1/library/refresh_cache` | 按范围刷新派生缓存（补缺失 + 消失对账） |
| POST | `/api/v1/library/cleanup_index` | 索引体检：清除隐藏文件/ignore 命中/源文件已删的残留位置 |

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
    "scan": { "periodic": true, "interval": 900 }
  }
}
```

素材元数据固定使用 `.hawk/metadata.db`（SQLite 单一路线），不再有 `storage_mode` 字段或切换端点；详见 [存储设计](storage.md)。

### update

`PATCH /api/v1/library/info`

改库显示名：写库内 `.hawk/config.toml` 的 `name` 键（toml_edit 保注释，保存即热更）；空白名清除自定义名（回退库目录名）。成功后就地广播 `library.updated`（负载为完整库信息），各客户端对齐无需重拉。响应同 `info`。

#### 请求

| 参数 | 类型   | 必填 | 说明         |
| ---- | ------ | ---- | ------------ |
| name | string | 是   | 新显示名     |

### scan

`PUT /api/v1/library/scan`

周期兜底重扫设置：写库内 `.hawk/config.toml` 的 `[scan]` 段（toml_edit 保注释，保存即热生效——消费循环每 30s 检查一次配置）。返回更新后的库信息并广播 `library.updated`。

语义：**实时监听仍是发现新增/删除的主路径**；周期重扫只是监听静默漏事件时的最终一致兜底（强制遍历、按 size/mtime 复用哈希，不读文件内容）。关闭后漏掉的事件需手动 `POST /library/rescan`（可带 `path`）收敛。

#### 请求

| 参数 | 类型 | 必填 | 说明 |
| ---- | ---- | ---- | ---- |
| periodic | bool | 是 | 是否开启周期兜底重扫 |
| interval | number | 否 | 重扫间隔（秒，下限 60）；缺省沿用当前值 |

#### 响应

同 `GET /library/info`（含 `scan` 字段）。

### reindex

`POST /api/v1/library/reindex`

全量重建索引：重新扫描素材目录并对所有文件重算哈希。异步执行，立即返回；过程中的变更照常通过 `events` 推送。用于 hawk 未运行期间直接改动过素材目录、或对索引状态存疑时手动触发。

#### 响应

```json
{ "status": "success" }
```

### rescan

`POST /api/v1/library/rescan`

重新扫描（文件系统层）：忽略文件夹快照强制重新遍历文件做复用判定（不读文件内容），收敛监听漏事件与直接改目录。异步执行，立即返回；进度经 `task.progress`（`index` 任务）可见，新增/删除经 SSE 事件自动刷新。

请求体必填，可为 `{}`；带 `path` 时只重扫该子树（含子目录）：**范围内做消失对账**（监听漏掉的删除一并收敛），但不做全局对账与目录快照替换（只看到子树，全局对账会误删其余位置），该子树的目录快照在下一轮全库扫描时自然收敛。范围目录不可读/已删除时跳过消失对账（防误删）。

#### 请求

| 参数 | 类型   | 必填 | 说明 |
| ---- | ------ | ---- | ---- |
| path | string | 否   | 库内相对路径（文件夹）；缺省/空 = 整库。不存在返回 `FOLDER_NOT_FOUND`，非法路径返回 `INVALID_PARAM` |

#### 响应

```json
{ "status": "success" }
```

### refresh_cache

`POST /api/v1/library/refresh_cache`

按范围刷新派生缓存（**补缺失模式**）：对范围内全部 item 派发后台修复任务——补缺失宽高（`0 × 0`）+ 生成缺失尺寸缩略图 + 提炼缺失调色板，**不重建已有文件**。异步执行立即返回，积压经 `task.progress`（`thumbnail` 任务）可见；修复项经 `item.updated` 事件自动刷新。

用户遇到显示异常（卡片 `0 × 0`、缩略图 404 占位不恢复等）时的手动修复入口。范围选项对应前端侧栏的右键菜单：文件夹（含子目录）/ 分类 / 标签 / 整库。宽高与缩略图缺失平时也会被读取端与周期对账自动自愈，此端点用于立即触发。

#### 请求

| 参数  | 类型   | 必填 | 说明                                                                 |
| ----- | ------ | ---- | -------------------------------------------------------------------- |
| type  | string | 是   | `folder` \| `category` \| `tag` \| `library`                          |
| value | string | 否   | folder/category/tag 的名称（folder 为目录相对路径，空串 = 库根）；library 时忽略 |

#### 响应

```json
{
  "status": "success",
  "data": { "dispatched": 42, "removed": 0 }
}
```

`dispatched` 为实际入队的修复任务数（in-flight 去重丢弃或源文件已不在的不计）；`removed` 为消失对账移除的失效位置数（源文件已删但索引残留的卡片，经 SSE 推送收敛）。

### cleanup_index

`POST /api/v1/library/cleanup_index`

索引体检：清除不该在索引里的条目——隐藏文件（路径任一段以 `.` 开头，如 `.DS_Store`）、ignore 规则命中或扩展名不在可见白名单内、源文件已删除的残留。**只摘索引位置不动磁盘文件**，移除经流水线单写者执行并广播事件（UI 自动收敛）；回收站条目不参与。与 `refresh_cache` 的差异：后者只对账源文件消失，本端点还清「文件还在但不该入库」的早期版本残留（增量扫描靠目录快照跳过 clean 目录，不会再触达它们）。多路径素材只摘命中位置，其余位置保留时条目仍在。同步执行后返回统计。

#### 响应

```json
{
  "status": "success",
  "data": { "checked": 120, "removed": 3, "hidden": 1, "ignored": 1, "missing": 1 }
}
```

| 字段 | 说明 |
| ---- | ---- |
| checked | 检查的索引位置数（不含回收站） |
| removed | 清除的条目总数（= hidden + ignored + missing） |
| hidden | 隐藏文件命中数 |
| ignored | ignore 规则命中或扩展名不在可见白名单内（`.hawk/config.toml` 的 `extensions`） |
| missing | 源文件已消失的残留数 |

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

删除文件夹：将目录（含其中素材）整体移入 `.hawk/trash/`。目录已不存在时按幂等删除处理：清除该路径前缀下的索引位置与目录设置残留（排序偏好/全局隐藏项）并返回成功（外部删除后的陈旧条目可经此入口清理）。

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
| POST | `/api/v1/item/duplicates`        | 重复图片分组扫描（相似 pHash / 相同内容多位置，admin） |
| POST | `/api/v1/item/aggregate`          | 选择集共有特性聚合：`{ ids }` → 标签/分类交集（多选面板数据源） |
| GET  | `/api/v1/item/detail`            | 获取单个 item  |
| GET  | `/api/v1/item/count`             | 获取 item 总数 |
| POST | `/api/v1/item/add`               | 添加新 item    |
| POST | `/api/v1/item/upload`             | multipart 上传新 item（web 端） |
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
| similarity_distance | number? | 仅相似查询：与源图的 pHash 汉明距离（0–64，越小越相似） |

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
| exclude_folders | string[] | 排除文件夹（含子目录，任一命中即剔除；空字符串条目被忽略）      |
| star       | number   | 按评分过滤                                                      |
| folders    | string[] | 按文件夹路径过滤（含子目录）                                    |
| folders_exact | boolean | 为 true 时文件夹只精确匹配直接位于该目录下的 item（不含子目录）；空字符串表示库根目录，默认 false |
| without_categories | boolean | 只返回未分类（没有任何分类）的 item，默认 false      |
| without_tags | boolean | 只返回未标签（没有任何标签）的 item，默认 false      |
| ext        | string   | 按扩展名过滤                                                    |
| annotation | string   | 按备注文本过滤                                                  |
| url        | string   | 按来源网址过滤                                                  |
| min_side_gte | number | 分辨率档位筛选（短边 = min(width, height)，像素）：≥ 阈值，可与 `min_side_lte` 组成区间 |
| min_side_lte | number | 分辨率档位筛选（短边 = min(width, height)，像素）：≤ 阈值，可与 `min_side_gte` 组成区间；宽高未解析（0×0）的 item 两方向均不命中 |
| min_width / max_width | number | 宽度独立区间（像素）：≥ / ≤ 阈值，两者可组合；与高度条件独立，宽未解析（0）时不命中 |
| min_height / max_height | number | 高度独立区间（像素）：≥ / ≤ 阈值，两者可组合；与宽度条件独立，高未解析（0）时不命中 |
| color      | string   | 按颜色检索（`#344441`，`#` 可省略，大小写不敏感）；命中条件为调色板任一颜色 CIE76 ΔE ≤ 25，格式非法返回 `INVALID_PARAM` |
| similar_to | string | 以指定 item 为源做 pHash 相似查找；源必须可见且已完成分析 |
| max_distance | number | 相似查询最大汉明距离（0–64），缺省 5；越小越严格 |
| in_trash   | boolean  | 是否只查回收站中的 item，默认 false                             |
| order_by   | string   | 排序字段：`modification_time`（默认）/ `name` / `size` / `star` / `similarity`（需同时指定 `similar_to`） |
| order      | string   | 排序方向：`desc`（默认）/ `asc`                                 |
| offset     | number   | 分页偏移，默认 0                                                |
| limit      | number   | 分页大小，默认 50                                               |

相似图片查找的算法、阈值和边界见 [pHash 相似图片查找](similar-images.md)。

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

### duplicates

`POST /api/v1/item/duplicates`（admin 限定）

扫描并分组重复图片，供「设置 → 存储 → 重复图片」使用。只返回分组与位置信息，不执行删除。

#### 请求

| 参数 | 类型 | 必填 | 说明 |
| ---- | ---- | ---- | ---- |
| kind | string | 否 | `similar`（pHash 近重复，默认）/ `exact`（同 item 多库内位置） |
| max_distance | number | 否 | 仅 similar：最大汉明距离 0–64，缺省 5 |
| offset | number | 否 | 组偏移，默认 0 |
| limit | number | 否 | 每组页大小，默认 50、最大 100 |

#### 响应

```json
{
  "status": "success",
  "data": {
    "groups": [
      {
        "id": "9b1f2c...",
        "kind": "similar",
        "entries": [
          { "item": { "id": "9b1f2c...", "path": "posters/a.png", "width": 1920, "height": 1080, "size": 245760 }, "similarity_distance": 1 },
          { "item": { "id": "71ab90...", "path": "posters/a-export.png", "width": 1920, "height": 1080, "size": 180120 }, "similarity_distance": 1 }
        ]
      }
    ],
    "total": 1,
    "offset": 0,
    "limit": 50
  }
}
```

- `total` 为分页前总组数，分页顺序稳定（组大小降序、组 id 升序）。
- exact 组只包含当前用户可见、非回收站位置数大于 1 的 item；similar 组只包含不同 item 的近重复集合，组内各位置完整展开。
- 锁过滤与其他查询一致；被锁定内容不会进入响应。
- 前端逐项显示文件大小、像素和路径，按 `id + path` 选择并调用 `item/delete` 移入回收站，不进行整 item 删除。

### skeleton

`POST /api/v1/item/skeleton`

请求参数与 `item/list` 完全相同（`offset` / `limit` 可省略，忽略），过滤、排序与 `item/list` **逐位一致**，但不分页，只返回布局所需的最低字段。供前端虚拟网格一次性建立完整布局（滚动条总高即时确定，可自由拖动跳转），视口内再按 offset 用 `item/list` 取详情。

#### 响应

```json
{
  "status": "success",
  "data": {
    "items": [
      { "id": "9b1f2c...", "path": "posters/a.jpg", "width": 1920, "height": 1080, "star": 4, "size": 1048576 }
    ],
    "total_size": 314572800
  }
}
```

条目数（`items` 长度）即全量计数；`star` 供网格 ★ 角标在未加载详情时显示；`size` 为该位置字节数，供选择集大小聚合（选择集可远超视口窗口，详情缓存承担不起全量聚合）。

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

向素材库添加新文件。`path`、`img_base64` 二者必须提供其一，作为文件内容来源；广域网受限写会话只接受 `img_base64`，携带 `path` 返回 403 `READ_ONLY`（防止远端读取服务器本机文件入库）；文件将写入 `folder_path` 指定的真实目录（缺省为库根目录），随后由索引流水线完成哈希与缩略图。来源网页（图片所在的页面地址）经 `website` 传入并记录为 Item.url。

**入库策略前置校验**（写盘前拒绝，避免落盘后又被入库判定剔除）：目标路径命中 `ignore` 规则返回 `INVALID_PARAM`；扩展名不在可见白名单（`.hawk/config.toml` 的 `extensions`）返回 `UNSUPPORTED_FORMAT`。`item/upload` 与 `item/update` 的改名/移动目标同样受此约束。

`path` 导入时保留原文件的创建时间与修改时间（`File.Copy` 默认会重置）：按 `modification_time` 排序与文件管理器观感均以原文件为准；`img_base64` 无原文件时间，取入库时刻。

#### 请求

| 参数        | 类型     | 必填   | 说明                                   |
| ----------- | -------- | ------ | -------------------------------------- |
| path        | string   | 二选一 | 本地文件路径，导入该文件               |
| img_base64  | string   | 二选一 | Base64 编码的图像数据                  |
| name        | string   | 否     | 文件名（不含扩展名），缺省取来源文件名 |
| folder_path | string   | 否     | 目标文件夹路径，缺省为库根目录         |
| tags        | string[] | 否     | 标签                                   |
| categories  | string[] | 否     | 分类（扁平名称校验；浏览器拖拽收集用）   |
| annotation  | string   | 否     | 备注                                   |
| website     | string   | 否     | 来源网页（Eagle `website` 同义），记录为 Item.url |
| skip_existing | boolean | 否     | 内容已存在于库内（不含回收站）时跳过：不写文件、不追加路径，响应 `skipped: true`（默认 false，即始终写入） |

#### 响应

Item 对象，并附带 `already_existed` / `skipped` 标志：

- 内容不存在：写入文件并索引，返回新 item，`already_existed: false`
- 内容已存在（同哈希）且未 skip：仍将文件复制到 `folder_path` 目标位置，已有 item 的 `paths` 追加新路径，返回该 item，`already_existed: true`——客户端可据此提示「内容已存在，已关联到现有条目」
- 内容已在库内且 `skip_existing: true`：不写入，`skipped: true`（前端导入的重复策略弹窗用）；内容仅存在于回收站时不视为重复，正常导入（删掉的内容应可重新导入）

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

### upload

`POST /api/v1/item/upload`

multipart/form-data 上传新 item（web 端 / iOS 分享扩展用）：客户端无本地文件路径可引用（拿到的是文件内容），经本端点以内容入库。写权限：admin 恒可用；viewer 需对应访问密钥 `write=true`（否则 403 `READ_ONLY`）。分类/标签可在同一个请求里带上，省去上传后再 `item/update` 的一次往返。

#### 请求（multipart/form-data）

| 字段        | 类型   | 必填 | 说明                                   |
| ----------- | ------ | ---- | -------------------------------------- |
| file        | binary | 是   | 文件内容；文件名只取末段（防跨目录），扩展名决定入库类型（与 path 导入同语义，不校验内容） |
| folder_path | text   | 否   | 目标文件夹路径，缺省为库根目录         |
| name        | text   | 否   | 文件名（不含扩展名），缺省取 file 文件名 |
| skip_existing | text | 否   | 传 `"true"` 时内容已在库内则跳过（同 item/add） |
| tags        | text   | 否   | 标签：同一字段名重复出现多次（multipart 无数组类型） |
| categories  | text   | 否   | 分类：同上；名称归一化去重（与 `item/add` 同语义） |

#### 响应

与 `item/add` 相同（Item + `already_existed`）。请求体上限 256MB。

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
- `remove_tags` / `remove_categories` 是**并集移除**（多选面板的共有特性摘除）
- `star` / `folder_path` 是**设置**(与 `item/update` 同语义)

#### 请求

| 参数          | 类型     | 必填 | 说明                                             |
| ------------- | -------- | ---- | ------------------------------------------------ |
| ids           | string[] | 是   | item id 列表(重复 id 自动去重)                 |
| add_tags      | string[] | 否   | 追加标签(并集,自动登记注册表)                  |
| add_categories | string[] | 否  | 追加分类(并集;名称校验同 `item/update`,自动登记注册表) |
| star          | number   | 否   | 评分 0–5(设置)                                 |
| remove_tags   | string[] | 否   | 移除标签(并集)                                |
| remove_categories | string[] | 否 | 移除分类(并集)                                |
| folder_path   | string   | 否   | 移动到该文件夹(移动各 item 的主位置;空字符串为库根目录) |

六个更新字段至少提供一个,否则返回 `INVALID_PARAM`。

#### 部分失败语义

批量操作不整体失败,逐项跳过:

- **内容不存在**的 id:跳过该项(元数据与移动都不应用),记入 `missing_ids`
- **移动冲突**(目标位置已有同名文件):跳过该项的移动,记入 `missing_ids`;`add_tags` / `add_categories` / `star` 照常应用
- **回收站中的 item**(无库内位置):移动不适用,跳过;元数据照常应用
- **空操作**（如追加的标签已存在，元数据无变化）：跳过落盘与事件，不计入 `updated`

#### 响应

```json
{
  "status": "success",
  "data": { "updated": 498, "missing_ids": ["abc...", "def..."] }
}
```

`updated` 为实际变更元数据的 id 数（空操作不计）;`missing_ids` 为上述未达成的 id(去重)。客户端可据此提示「已更新 n 项,m 项未处理」。

事件推送合并为分块的 `items.updated`（每块最多 1000 个 item），不再逐条发 `item.updated`——大批量（数万项）时逐条事件会形成风暴打挂客户端。

### delete

`POST /api/v1/item/delete`

移入回收站：文件移入 `.hawk/trash/`（保留目录结构），元数据保留。不带 `path` 为卡片级删除：回收该 item 的**全部**库内位置（同内容多路径 item 只回收一个位置会便卡片残留在网格）；带 `path` 为单位置删除。

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

从回收站恢复：文件移回元数据 `paths` 记录的原路径。不带 `path` 恢复全部回收站位置（与 delete 对称）；原路径已被占用的位置跳过留在回收站，全部冲突才返回 `FILE_EXISTS`。

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

`GET /api/v1/item/thumbnail?id=<hash>&size=512`

| 参数 | 类型 | 必填 | 说明 |
| ---- | ---- | ---- | ---- |
| id | string | 是 | item 内容哈希 |
| size | number | 否 | `512`（网格）或 `1024`（检查器/预览），缺省 `1024`；其他值返回 400 |

缩略图为固定 512/1024 两档 WebP 缓存（等比缩入边长内，不放大）。**扫描导入即时生成**：首扫并行阶段单次解码同出全部缺失尺寸+调色板+宽高；增量/对账不生成，由读取端兜底。旧库已有 1024 档时，补 512 档直接解码 1024 缓存，无需重读原图。

- **命中请求尺寸** → 200，返回对应 `image/webp`，带 `Cache-Control: public, max-age=31536000, immutable`
- **未命中** → 后台入队生成全部缺失档并返回 404；完成后经 `item.updated` 通知客户端重建 `<img>` 重试
- **不回源原图**：缩略图端点只返回 WebP，避免低带宽下按卡片下载整份原图，也避免 immutable 缓存永久钉住原图回退响应

### file

`GET /api/v1/item/file?id=<hash>`

返回原图文件二进制，Content-Type 按扩展名推断（无法识别时为 `application/octet-stream`）。本地/局域网预览浮层直接用它；广域网默认使用 1024 缩略图，仅在放大、保存、复制或编辑时请求原图。文件位置取 item 主位置（优先非回收站位置）；文件已缺失时返回 404。与缩略图同理带 `Cache-Control: immutable`。`<img>` 直链无法设置请求头，与 thumbnail 一样放行查询参数 `?token=`。

### refresh_thumbnail

`POST /api/v1/item/refresh_thumbnail`

手动强制重建缩略图：对指定 item 的全部尺寸强制重新生成（不走「已存在跳过」）。派发后台任务异步执行，立即返回；完成后经 `item.updated` 事件通知前端重建 `<img>`。任务同时补全缺失宽高与调色板（与 `library/refresh_cache` 的修复逻辑一致，区别在于强制重建已有文件）。

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

## global_filter

全局列表隐藏项（`.hawk/global_filter.toml`，参与同步）：被标记的文件夹/分类/标签，其下素材由客户端在「全部素材/根目录/未分类/未标签」等全局视图查询时附带 `exclude_*` 参数排除（OR 语义：命中任一隐藏维度即剔除）；维度自身视图与回收站不排除。文件夹条目为库内相对路径，子树整体隐藏。

级联跟随与排序偏好同款：文件夹移动/重命名（含移入回收站，恢复时回归）自动迁移，删除/清空回收站自动清除；分类/标签重命名跟随（目标已隐藏时合并）、删除清除。变更广播 `global_filter.changed`（含外部同步写入的重载），负载为完整快照。

| 方法 | 端点 | 说明 |
| ---- | ---- | ---- |
| GET | `/api/v1/global_filter/list` | 全部隐藏项：`{ "folders": [...], "categories": [...], "tags": [...] }` |
| PUT | `/api/v1/global_filter` | `{ "kind": "folder" \| "category" \| "tag", "name", "hidden" }`，幂等；路径/名称非法返回 `INVALID_PARAM` |

## lock

文件夹/分类/标签锁（`.hawk/locks.toml`，参与同步）：被锁维度的内容需要密码解锁后才可见，由**服务端强制**（与 global_filter 的客户端约定式排除不同）：

- 主动筛选（`item/list`、`item/skeleton` 的 `folders`/`categories`/`tags` 参数）命中未解锁的锁 → `403 LOCKED`
- 全局视图：未解锁锁覆盖的条目由服务端直接排除（同查询同排序，skeleton 与 list 一致）
- 内容直连（`item/thumbnail`、`item/file`、`item/detail`、`item/aggregate`）命中未解锁的锁 → `403 LOCKED`（防止拿到 hash 直接拼 URL 绕过）

判定语义（OR 剔除）：条目命中任一未解锁的锁即不可见；同内容多路径时，存在任一「无锁或已解锁」位置即位置维度放行，分类/标签锁与位置无关（item 属性）。锁定文件夹移入回收站后保持锁定（条目随路径迁移）。**锁不是加密**：文件系统层面无保护，仅防止「通过 hawk 查看」；密码哈希（Argon2id）参与同步，拿到 `.hawk/` 的人可离线爆破（慢哈希缓解）。

**解锁票据**：`lock/unlock` 校验密码后发放随机票据（daemon 内存态，重启失效），由各客户端独立持有——解锁状态不随分享的链接扩散。后续请求经 `X-Hawk-Unlock: <t1>,<t2>` 请求头附带；`<img>` 直链与 SSE 无法设头，改用查询参数 `?unlock=<t1>,<t2>`（路径集合与 token 查询参数通道一致）。**不要把带 unlock 参数的图片 URL 分享给他人**（等同分享该内容的解锁能力）。前端丢弃票据即「锁定回去」。

级联跟随与 global_filter 同款：文件夹移动/重命名（含移入回收站，恢复时回归）自动迁移，删除/清空回收站自动清除；分类/标签重命名跟随（目标已锁时合并，保留目标密码）、删除清除。变更广播 `locks.changed`（含外部同步写入的重载）。验证失败全局节流：连续 5 次失败冷却 60s（`429 THROTTLED`）。

| 方法 | 端点 | 说明 |
| ---- | ---- | ---- |
| GET | `/api/v1/lock/list` | 全部锁（仅名称，不含密码哈希）：`{ "folders": [...], "categories": [...], "tags": [...] }` |
| POST | `/api/v1/lock/set` | `{ "dimension": "folder" \| "category" \| "tag", "name", "password", "old_password"? }`；已锁条目改密必须带正确旧密码（否则 `403 OLD_PASSWORD_REQUIRED`）。admin 限定 |
| POST | `/api/v1/lock/remove` | `{ "dimension", "name", "password" }`，解除锁需密码。admin 限定 |
| POST | `/api/v1/lock/unlock` | `{ "dimension", "name", "password" }` → `{ "unlock_token": "..." }`；任何有效 token（含只读 viewer）可解锁；锁不存在 `404 LOCK_NOT_FOUND`、密码错误 `401` |

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
| `items.updated`   | `{ "items": [Item...] }` | `item.updated` 的批量变体（调色板批量回写、`batch_update`、分类/标签重命名与删除的级联迁移），攒满 1000 条发一帧，客户端逐个就地替换缓存 |
| `item.trashed`    | `{ "id": "..." }` | 最后一个库内位置移入回收站 |
| `item.restored`   | Item 对象 | 首个回收站位置回归库内 |
| `item.removed`    | `{ "id": "..." }` | 彻底删除(无剩余位置) |
| `folder.changed`  | `{ "reason": "external" }` | 目录结构可能变化,客户端应重拉 `folder/list`;reason 恒为 `external`,客户端必须忽略取值(结构为将来预留) |
| `library.updated` | LibraryInfo 对象 | 库显示名变更（`PATCH library/info`）；负载为完整库信息，客户端就地替换 |
| `global_filter.changed` | `{ "folders": [...], "categories": [...], "tags": [...] }` | 全局列表隐藏集变更（标记/取消、级联跟随、外部同步重载）；负载为完整快照，客户端就地替换并重查列表 |
| `locks.changed` | `{ "folders": [...], "categories": [...], "tags": [...] }` | 锁集变更（设锁/解除/改密、级联跟随、外部同步重载）；负载为名称快照，客户端重拉锁标记并重查列表。锁覆盖内的素材不发 `item.*` 事件（保守过滤，已解锁客户端可能漏收，靠解锁后的重查兕底） |
| `remote.session` | `{ "session_id": "s_9", "target_device_id": "d_42", "state": "connected" }` | 远程会话生命周期（connected/disconnected）：隧道建立或断开时推送，app 据此切换连接描述符与断线自动回切本地库 |
| `remote.status` | `{ "state": "online", "server_url": "...", "device_id": "d_42" }` | 信令状态变化（disabled/connecting/online/error），远程设置面板免轮询；error 时负载带 `error` 原因 |
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

触发来源:本端文件夹增删改移(API)、外部进程目录操作(文件监听)、周期对账扫描兜底。文件夹树无增量语义(前端经 `folder/list` 拉全量树;服务端按目录结构缓存建树、计数实时合并,缓存失效与本事件同线),事件只表达「需要重拉」。

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

- **节流**:`task.progress` 服务端 500ms 最多一帧;`item.*` 事件无节流;批量元数据操作（`item/batch_update`）合并为分块的 `items.updated` 帧
- **不保证送达**:订阅者消费跟不上(积压 1024 条)时服务端直接断开该订阅——客户端重连后必须以 `item/skeleton` + `folder/list` 全量对齐,不得假设收到过全部事件
- **初始索引期间**:就绪网关拦截期内不推事件(订阅端点同样 503),主界面加载完成后订阅即可
- **顺序**:同一 item 的事件按流水线处理顺序发出;不同 item 之间无全局顺序保证
- **重连**:EventSource 断线自动重连,`onopen` 再次触发即对齐时机(参考前端 `events.ts` 的 `onReconnect` 约定)


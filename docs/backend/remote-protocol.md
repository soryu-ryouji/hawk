# 远程访问协议 V1（remote-protocol）

> hawk 客户端（hawk-daemon 的 remote 模块，见 [remote-access.md](remote-access.md)）与远程访问服务端（hawk-server，本仓库 `hawk-server/`）之间的接口契约。协议是两侧的唯一共享物，实现零代码共享。
> 多租户方案：一台 hawk-server 可服务多个池（租户）。两级凭证——**service-key**（池门禁，程序生成，server 侧 `tenant add` 创建）与 **hawk-app key**（设备本地签发，逐把读/写权限，分发给网页版/插件等查看者）。协议无账号/会员/计费概念；账号系统后续接入时只替换 service-key 的签发水源，协议不动。访问被拒的语义为凭证无效（401）。
> 总体架构见 [remote-access.md](remote-access.md)。
> **传输层为 [iroh](https://github.com/n0-computer/iroh)**（QUIC/noq + magicsock：打洞、中继、路径迁移、映射保活内建）。控制面只负责撮合与凭证——候选组装、打洞时序、中继回落等传输细节不进本协议；控制面交换的寻址信息是 iroh 的 `EndpointAddr`（endpoint_id + relay_url + 直连地址列表）。

## 凭证模型

```text
service-key（池门禁）
  ├─ hawk-app（daemon remote 模块）：注册/心跳/会话/控制面全部用它（Bearer）
  └─ 查看者第一级：POST /viewer/gate 换门令牌（gt，5 分钟、5 次尝试预算）

hawk-app key（逐 key 权限：读 / 写 / 读+写）
  └─ 查看者第二级：POST /viewer/login（门令牌 + key）换浏览器令牌 bt
       bt 绑定唯一目标设备与作用域（read/write），查看面专用，不参与控制面鉴权
```

**存储与验证**：

- service-key：`hawk-server tenant add` 程序生成（`hs_` + 256-bit 随机），明文只打印一次；服务端只存 `HMAC-SHA256(pepper, key)`。pepper 存数据目录 `pepper` 文件（0600，首启自生成），注册表泄露但没有 pepper 时哈希不可逆推 key
- hawk-app key：daemon 本地签发保管（默认 `hk_` + 128-bit 随机；允许自定义，纯读 ≥4 字符、带写权限 ≥12 字符），**明文只在签发时返回一次**；daemon 本地存 `sha256(key)`（验证子 s），server 经 `PUT /devices/{id}/keys` 同步 `{ s, read, write }` 并只存 `HMAC-SHA256(pepper, s)`——server 全程不见 key 明文
- 查看者登录时 server 计算 `HMAC(pepper, sha256(出示的 key))` 比对；比对命中后 bt 记录 `{ access, device_id, credential: s }`
- 会话执法在目标 daemon：查看者会话的 `session.request` 携带 `credential`（s），daemon 比对本地 key 映射裁决权限——server 被攻破也无法把只读 key 放大成写权限；app 间会话（无凭证）的写权限只由目标 daemon 的 `remote.allow_import` 总开关裁决（单用户租户内设备互信）
- 比较一律恒定时间

**风控（限速）**：凭证验证面按来源 IP 与租户（池）双维度计失败：

- 单 IP：gate/login/控制面鉴权失败窗口内（10min）超 30 次 → 冷却 15min（429）
- 单租户：login 失败窗口内超 20 次 → 整池冷却 15min，连续触发指数增长封顶 24h；一次成功登录清零
- 门令牌：TTL 5 分钟，最多验证 5 次 hawk-app key

**多租户隔离**：设备/会话/key 注册表按租户（池）隔离；跨池访问一律 404（不泄露他池存在性）。「网页版」服务端设备是全局设备（不属于任何池），不出现在任何租户的设备列表中。

## 通用约定

- 控制面前缀 `/api/v1`；服务端地址 `<base>` 由部署者决定
- 请求与响应字段 snake_case，时间戳 Unix 毫秒
- 响应信封沿用 hawk 本体约定，错误信封不带业务错误码——语义由 HTTP 状态码承载，`message` 供客户端直接展示：

```json
{ "status": "success", "data": {} }
{ "status": "error", "error": { "message": "服务密钥无效" } }
```

### HTTP 状态码（即全部错误语义）

| 状态码       | 含义                                                                              |
| ------------ | --------------------------------------------------------------------------------- |
| 401          | service-key / hawk-app key / 浏览器令牌无效                                       |
| 429          | 风控冷却中（响应 message 含剩余秒数）或心跳超速                                    |
| 其他 4xx/5xx | 按 HTTP 惯例（400 参数、403 无权操作、404 不存在/不在线、409 会话已结束、500 服务端错误） |

- 控制面鉴权头：`Authorization: Bearer <service_key>`。**WS 升级请求同样用该头**——key 一律不走 URL 查询串（反代 access log 会记录完整 URL）
- **device_id**：设备注册时由服务端分配，客户端持久化，随请求携带。客户端设备身份是**应用级**的（不随素材库切换）；同一设备切换素材库后由心跳更新 `library_name`
- **endpoint_id**：设备的 iroh 身份（Ed25519 公钥，hex 编码）。iroh 握手即按公钥认证——没有单独的证书钉扎环节；轮换即换钥，旧身份的会话随即失效
- 控制面全量 TLS：由前置反代（Caddy 等）终结，hawk-server 进程监听明文 HTTP
- 限速：心跳最小间隔 10s（违反返回 429）；凭证验证面限速见「凭证模型」

## 接入配置（协议视角）

**hawk-app 的最小知识 = 一个 URL + 一把 service-key；查看者的最小知识 = URL + service-key + 一把 hawk-app key。**

- 部署者用 `hawk-server tenant add --label <名>` 创建租户并拿到 service-key（明文只显示一次），分发给自己各台设备的 hawk-app；`tenant list` / `tenant revoke` 管理（吊销即该池 service-key 与池内 hawk-app key 映射全部失效）
- 客户端接入流程：`POST /devices` 注册设备（拿 device_id）→ 持久化 → `PUT /devices/{id}/keys` 同步 key 映射 → 心跳保活 → `GET /devices` 拉本池设备列表 → 发起会话
- service-key 吊销/更换后所有请求返回 401，客户端重新配置即可

## 设备管理

设备注册表持久化于服务端数据目录（`devices.json`，含 `tenant_id` 池归属），会话与在线状态为内存状态，服务端重启后设备需重新心跳上线。

### `POST /api/v1/devices`

注册设备（service-key 鉴权，设备入调用者所在池），返回 device_id。

```json
// 请求
{
  "name": "家里台式机",
  "platform": "windows",
  "endpoint_id": "<hex，iroh EndpointId（Ed25519 公钥）>",
  "library_name": "我的素材库",
  "allow_remote": true,
  "allow_import": false
}
// 响应
{ "status": "success", "data": { "device_id": "d_42" } }
```

`allow_remote`：设备级开关，关闭时该设备的会话请求被直接置 `rejected`（UI 里可改）。
`allow_import`：目标设备当前是否允许广域网写会话（对应应用级 `remote.toml` 的 `writable`）；由设备注册/心跳上报，服务端用于写会话预检，最终由目标 daemon 按本地 key 映射每请求裁决。
`library_name`：设备当前打开的素材库名。设备身份跨库共享，同一台设备切换库后经心跳更新设备表，避免列表展示旧库名。

### `POST /api/v1/devices/heartbeat`

设备在线心跳，携带寻址信息。间隔 30s；服务端 90s 未收到心跳判离线。设备须属调用者池（他池设备 404）。

```json
// 请求（service-key 鉴权；device_id 标识本机）
{
  "device_id": "d_42",
  "allow_import": false,
  "library_name": "我的素材库",   // 当前打开的库名；服务端据此收敛设备表（可省略）
  "addrs": [                      // iroh 上报的直连地址集（本机接口 + 端口映射/外部地址；
    "192.168.1.5:27373",          // 随网络环境刷新，心跳周期自然收敛），空数组 = 无直连地址
    "1.2.3.4:27373"
  ]
}
// 响应
{ "status": "success", "data": {
    "server_time_ms": 1750000000000,
    "public_ip_seen": "1.2.3.4"   // 服务端看到的连接源地址（诊断用）
} }
```

寻址的权威性说明：客户端上报的是自己视角的直连地址，服务端不验证其可达性——**拨号方的连接尝试即最终校验**（不可达的地址 iroh 会自然跳过；提供中继时打洞/回落由传输层兜底）。

### `GET /api/v1/devices`

本池设备列表（service-key 鉴权；不含「网页版」全局设备）。`online` 以心跳为准（90s 内心跳为在线）；`addrs` 为最近一次心跳的上报，未心跳过为空：

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
        "last_seen_at_ms": 1750000000000,
        "endpoint_id": "...",
        "allow_remote": true,
        "allow_import": false,
        "addrs": ["192.168.1.5:27373", "1.2.3.4:27373"]
      }
    ]
  }
}
```

### `PATCH /api/v1/devices/{device_id}`

更新设备（service-key 鉴权；他池设备 404）：

```json
// 请求（字段均可选）
{
  "name": "新名字",
  "allow_remote": false,
  "library_name": "工作库",
  "endpoint_id": "<新 endpoint_id>"
}
// 响应：更新后的设备信息（同 GET /devices 单条结构）
```

`endpoint_id` 更新即身份轮换：新身份立即生效，旧身份的会话随即失效。

### `DELETE /api/v1/devices/{device_id}`

注销设备（service-key 鉴权；他池设备 404）。其进行中的会话一并取消，对端经 WSS 收到 `session.cancelled`；该设备的 hawk-app key 映射一并清除。

### `PUT /api/v1/devices/{device_id}/keys`

全量同步本设备的 hawk-app key 映射（service-key 鉴权；覆盖式，daemon 在 key 变更与每次接入时推送）。daemon 只上送验证子 `s = sha256(key)`，server 不见明文：

```json
// 请求
{ "keys": [ { "s": "<sha256-hex>", "role": "viewer" } ] }
// 响应
{ "status": "success", "data": { "synced": 1 } }
```

校验：`s` 须 64 位小写十六进制；`role` 为 viewer/importer；任一条目非法整体拒绝（400）。

## 查看面登录（两级验证）

浏览器/插件等外部查看者经查看面 API 获得浏览器令牌 `bt`（`hb_` 前缀；服务端只存哈希、作用域、绑定设备与凭证）。查看面的其余端点（会话/代理）是服务端实现细节，不进本协议。

### `POST /api/v1/viewer/gate`

第一级：service-key 换门令牌。失败按来源 IP 计风控；冷却中 429。

```json
// 请求
{ "service_key": "hs_..." }
// 响应
{ "status": "success", "data": { "gate_token": "gt_...", "expires_in": 300 } }
```

### `POST /api/v1/viewer/login`

第二级：门令牌 + hawk-app key 换 bt。消耗门令牌一次尝试；失败按 IP 与租户双维度计风控；成功清零该租户失败计数。

```json
// 请求
{ "gate_token": "gt_...", "key": "hk_..." }
// 响应：bt 绑定唯一目标设备（key 映射决定），access 为 read / write（write 含读）
{ "status": "success", "data": { "browser_token": "hb_...", "role": "importer", "device_id": "d_42" } }
```

key 无效、已吊销、或不属于门令牌绑定的池，同按 401「访问密钥无效」处理（不泄露存在性）。

## 控制通道（WSS）

设备建立 WebSocket 长连接接收服务端推送（会话请求、状态变更）。鉴权走 `Authorization: Bearer <service_key>` 头（**不走 URL 查询串**）：

```text
wss://<base>/api/v1/ws?device_id=<device_id>
```

消息统一信封：`{ "type": "...", ... }`。同一设备的新连接顶替旧连接（旧连接收到 Close）。

### 服务端 → 设备

| type                | 内容                                                                                   |
| ------------------- | -------------------------------------------------------------------------------------- |
| `hello`             | `{ "device_id", "server_time_ms" }`                                                    |
| `ping`              | `{}`（保活，30s 间隔，两次未回 pong 断开）                                              |
| `session.request`   | `{ "session_id", "from": { "device_id", "name", "library_name", "endpoint_id" }, "role": "viewer" \| "importer", "credential": "<sha256-hex 或 null>" }` |
| `session.update`    | `{ "session_id", "state", ... }`（状态变更推送双方；轮询为兜底）                         |
| `session.cancelled` | `{ "session_id" }`                                                                     |

`from.endpoint_id`：发起方的 iroh 身份（服务端持有双方设备记录，随请求下发，省一次设备列表往返）。`credential`：查看者会话的 key 验证子（app 间会话为 null）；目标 daemon 据此比对本地 key 映射执法——未命中/权限不足/本机 writable 关闭即拒绝。

### 设备 → 服务端

| type        | 内容 | 说明                 |
| ----------- | ---- | -------------------- |
| `hello_ack` | `{}` | 应答 hello，连接就绪 |
| `pong`      | `{}` | 保活应答             |

业务动作（心跳、会话响应）一律走 REST，通道保持单向简单。

客户端以服务端帧为通道活性信号：超过 90s（3 个 ping 周期）未收到任何服务端帧即判定连接半开，主动断开重连。中间盒静默断链（反代切换、NAT 会话老化）时上行 REST 心跳可能仍然通畅——不能以上行心跳自证在线，否则设备会永远「假在线」而收不到任何推送。

## 会话编排

### `POST /api/v1/sessions`

发起会话（service-key 鉴权；双方设备须同池，跨池 404）。请求可带 `role: viewer | importer`（缺省 viewer）；**无 mode 参数**——直连/打洞/中继的选择是传输层自治，不进控制面。`role=importer` 要求目标设备 `allow_import=true`（否则 403）。授权口径分两路：查看面会话由 hawk-server 以「网页版」全局设备身份发起、携带查看者凭证，目标 daemon 按本地 key 映射裁决；app 间会话（同租户设备，无凭证）由目标 daemon 按本机 `remote.allow_import` 裁决——单用户租户内设备互信，不再要求访问密钥。

```json
// 请求（access 可省略，缺省 read）
{ "source_device_id": "d_77", "target_device_id": "d_42", "role": "importer" }
// 响应
{ "status": "success", "data": { "session_id": "s_9", "state": "pending" } }
```

服务端流程：

1. 校验来源/目标设备存在且同池、目标设备在线（不在线返回 404）；`allow_remote=false` 直接置会话状态 `rejected`；`role=importer` 且目标 `allow_import=false` 返回 403
2. 经 WSS 向目标推送 `session.request`（含 `access`、查看者会话含 `credential`）
3. 目标回 `POST /api/v1/sessions/{id}/accept`（service-key 鉴权），响应 `{ "status": "success" }`；拒绝则回 reject，会话状态变 `rejected`。目标 daemon 本地执法：查看者会话查 key 存在性/权限，app 间写会话查本机 `remote.allow_import`，不通过即 reject
4. 服务端组装连接信息并置 ready，经 WSS `session.update` 推送双方（见下）

发起方以 WSS 推送为准（轮询 1s 间隔、上限 30s 为兜底；超时状态变 `expired`）：

### `GET /api/v1/sessions/{session_id}`

会话状态即全部结果语义：

```json
// pending
{ "status": "success", "data": { "session_id": "s_9", "state": "pending" } }
// ready
{ "status": "success", "data": {
    "session_id": "s_9", "state": "ready",
    "connect": {
      "endpoint_id": "<hex，目标设备的 iroh 身份>",
      "relay": { "url": "https://<base>:9102", "token": "<relay 准入 token>" },  // 服务端内嵌中继启用时存在
      "direct_addresses": ["192.168.1.5:27373", "1.2.3.4:27373"]                  // 目标最近心跳上报；可空
    }
} }
```

`connect` 即 iroh `EndpointAddr` 的控制面表达：`endpoint_id` + 直连地址 +（可选）中继。发起方据此拨号：

- 服务端提供中继（`relay` 字段在场）：拨号带上 `relay`——iroh 先经中继建连（首字节零等待），后台打洞成功后流量自动迁移到直连；打洞不成则一直在中继上。**中继同时是打洞协调通道：无中继的部署（connect 无 relay 字段）只有直连地址可达的会话能建立**
- 客户端设备级开关可选择不使用中继（拨号时丢弃 `relay` 字段并以 relay-disabled 形态重建 endpoint）；不需要逐次确认——是否使用中继只由「服务端是否提供 × 客户端是否接受」两个静态开关决定
- `endpoint_id` 即身份：iroh 握手按 Ed25519 公钥认证，无需额外钉扎

| state         | 含义                                          | 客户端行为                                                 |
| ------------- | --------------------------------------------- | ---------------------------------------------------------- |
| `pending`     | 等待目标设备响应                              | 继续等待                                                   |
| `ready`       | 连接信息已就绪                                | 建立隧道（iroh connect）                                   |
| `rejected`    | 目标设备拒绝（或 `allow_remote=false`，或查看者 key 未过本地执法） | 展示"对方拒绝"                                             |
| `unreachable` | 服务端未提供中继且目标无任何直连地址            | 提示当前网络不可达（开中继/换网络后重试）                   |
| `expired`     | 建立超时（30s）                               | 提供重试                                                   |
| `cancelled`   | 任一方取消（或设备注销）                      | 结束                                                       |

查看方本地代理（端口、会话随机 token）由发起方客户端自行生成，不进协议——隧道建立后由客户端拼装 web 查看器 URL（见数据面一节）。

### `POST /api/v1/sessions/{session_id}/accept`

目标设备接受会话（service-key 鉴权），请求体携带 `{ "device_id": "d_42" }` 标识本机，响应 `{ "status": "success" }`。仅 pending 状态可响应（重复响应或响应终态会话返回 409）；仅目标设备可响应（来源设备响应返回 403）；目标设备须属调用者池（跨池 404）。

### `POST /api/v1/sessions/{session_id}/reject`

目标设备拒绝会话（service-key 鉴权），请求体携带 `{ "device_id": "d_42" }` 标识本机；会话状态变 `rejected`。

### `POST /api/v1/sessions/{session_id}/cancel`

任一方取消会话（无请求体；会话双方之一须属调用者池）。pending/ready → `cancelled`，双方经 WSS 收到 `session.cancelled`；已 cancelled 幂等成功；其余终态返回 409。

## 中继（服务端可选组件）

hawk-server 内嵌 [iroh-relay](https://docs.rs/iroh-relay)（库形态，不另起进程）。对协议而言中继**没有独立端点**——它只是 `connect.relay` 字段的来源；客户端与 relay 之间的协议是 iroh 内部事务。

- **准入**：内嵌 relay 配置 shared token（服务端启动配置）；token 随 `connect.relay.token` 经控制面下发（控制面有 service-key 鉴权 + TLS，准入语义等价于池内成员）
- **开关**：`--relay-port 0` 关闭中继组件（该部署退化为纯直连：无 relay 字段、无打洞协调——只有直连地址可达的会话能建立）；客户端设备级开关可不使用中继
- **带宽语义**：中继承载的是端到端加密的 QUIC 数据报（服务端只见密文）；「是否使用中继」不再有逐次确认——自建带宽保护由两个静态开关承载（服务端可提供与否 × 客户端可接受与否）
- **网页版**：服务端的「网页版」宿主以全局设备身份作为发起方走同一会话流程（进程内 iroh endpoint 拨号），浏览器与服务器之间的查看面登录是服务端实现细节（见「查看面登录」）

## 数据面（iroh 隧道）

- 双方 iroh Endpoint（QUIC/noq + magicsock），身份 = `endpoint_id`（Ed25519 公钥，注册时登记，握手即认证）。轮换走 `PATCH /devices/{id}`：新身份立即生效，旧身份会话失效
- 打洞、路径迁移、NAT 映射保活、relay↔direct 升级降级全部由 iroh 自治；控制面不感知。连接死亡（迁移救不回）时发起方在原会话内重拨并热替换进会话代理，前端无感
- 隧道内 HTTP 请求必须携带会话标识头 `x-hawk-session: <session_id>`（B 侧代理写入，客户端自带的同名头一律覆盖）。A 侧校验会话已接受、且与该连接的对端 endpoint_id 一致后，在进程内插入 `TunneledPeer { writable }`；HTTP 客户端无法伪造该标记
- A 侧按会话权限转发到本机 API：
  - 只读会话：viewer 级，仅 GET 与查询类 POST
  - 写会话：`role=importer` 且请求时 `remote.allow_import=true`，且来源合法（查看者会话 = key 带写权限；app 间会话 = 同租户设备）；仅放行 GET、查询类 POST、`POST /api/v1/item/add`（禁止 `path`）、`POST /api/v1/item/upload` 与 `POST /api/v1/folder/create`，其余写端点 403
- **隧道端必须拒绝转发 `GET /api/v1/app/admin-token`（直接返回 404）**。该端点免鉴权返回主 token（完全读写权限），仅服务于本机浏览器插件的零配置发现（插件直连环回，不经隧道）；远程查看方不需要它——B 侧已持有代理会话 token
- B 侧本地代理：监听环回随机端口，每次会话随机 token，web 查看器以 `?token=` 携带；代理响应不带 CORS 头。代理端口由客户端会话建立时分配，会话 token 由 B 侧生成，web 查看器 URL 由客户端拼装，不经过服务端
- 同一设备身份可同时承载多个查看会话（例如 hawk-server 的网页版与插件）：A 侧按会话放行，连接与会话绑定，取消一个会话只关闭该会话的连接与请求
- 隧道端口：iroh endpoint 绑定 UDP 27373（可配置；双栈监听）；本地端口由客户端在会话建立时分配

## 默认值与时限

| 项                    | 值                                                                                              |
| --------------------- | ----------------------------------------------------------------------------------------------- |
| 心跳间隔 / 离线判定   | 30s / 90s                                                                                       |
| 心跳最小间隔          | 10s（429）                                                                                      |
| WSS 保活              | ping 30s 间隔，两次未回 pong 断开；客户端 90s 未收到服务端帧即主动断开重连                       |
| 会话建立超时          | 30s                                                                                             |
| 拨号超时              | 15s（客户端实现；iroh 内部竞速直连/中继，首字节不等打洞结果）                                    |
| 门令牌 TTL / 尝试预算 | 5min / 5 次                                                                                     |
| 风控                  | 单 IP 10min 内 30 次失败 → 冷却 15min；单租户 20 次登录失败 → 池冷却 15min 起指数增长（封顶 24h），成功清零 |
| 端口默认值            | hawk-server 9100 / 内嵌中继 9102（可选组件）/ hawk API 27371（用户可配）/ LAN 网站 TCP 27372 / hawk-app 直连 TCP 27375 / 远程隧道 UDP 27373（本地端口由客户端在会话建立时分配） |

## 版本策略

- 协议版本隐含于路径 `/api/v1`；不兼容变更升 v2 并保留 v1 过渡期
- 新增可选字段向后兼容：客户端忽略未知字段，服务端对缺失字段取默认值
- WSS 消息同理：未知 `type` 忽略

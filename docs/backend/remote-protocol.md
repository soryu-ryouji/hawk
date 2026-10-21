# 远程访问协议 V1（remote-protocol）

> hawk 客户端（hawk-remote 进程，见 [remote-access.md](remote-access.md)）与远程访问服务端（hawk-server，本仓库 `hawk-server/`）之间的接口契约。协议是两侧的唯一共享物，实现零代码共享。
> 单用户方案：服务端配置一把 access key 用于控制面与只读查看；可选一把 write key 用于浏览器插件的广域网写会话。协议无账号/会员/计费概念。访问被拒的语义为 key/token 无效（401）。
> 总体架构见 [remote-access.md](remote-access.md)。标注 `[P1]` 为直连控制面（已实现）；`[P2]` 为中继（已实现，见「中继」一节）。

## 通用约定

- 控制面前缀 `/api/v1`；服务端地址 `<base>` 由部署者决定（客户端配置 = 服务端 URL + KEY）
- 请求与响应字段 snake_case，时间戳 Unix 毫秒
- 响应信封沿用 hawk 本体约定，错误信封不带业务错误码——语义由 HTTP 状态码承载，`message` 供客户端直接展示：

```json
{ "status": "success", "data": {} }
{ "status": "error", "error": { "message": "访问密钥无效" } }
```

### HTTP 状态码（即全部错误语义）

| 状态码       | 含义                                                                              |
| ------------ | --------------------------------------------------------------------------------- |
| 401          | 访问密钥/浏览器令牌无效                                                             |
| 其他 4xx/5xx | 按 HTTP 惯例（400 参数、403 无权操作、404 不存在/不在线、409 会话已结束、429 限速、500 服务端错误） |

- 鉴权头：`Authorization: Bearer <access_key>`。控制面唯一凭据是 access_key：部署者启动服务端时经 `--access-key` / `HAWK_SERVER_ACCESS_KEY` 配置（建议 256-bit 随机值，服务端恒定时间比较）。客户端配置 = 一个 URL（服务端地址）+ 一个 KEY（访问密钥），无登录流程、无会话令牌
- 查看面令牌 `bt`：由 `POST /api/v1/viewer/login` 换发（`access_key` → read，可选 `--write-key` → write），服务端只存哈希与作用域。它只用于查看面，不参与控制面鉴权
- **device_id**：设备注册时由服务端分配，客户端持久化，随请求携带（用于心跳、会话响应、WSS 等需要标识设备身份的场合）
- 控制面全量 TLS：由前置反代（Caddy 等）终结，hawk-server 进程监听明文 HTTP
- 限速：心跳最小间隔 10s（违反返回 429）

## 接入配置（协议视角）

**客户端的最小知识 = 一个 URL + 一个 KEY。** 协议中没有登录端点：

- 部署者自行生成 KEY 并配置到服务端与全部客户端；需要隔离时可分设备部署多实例
- 客户端接入流程：`POST /devices` 注册设备（拿 device_id）→ 持久化 → 心跳保活 → `GET /devices` 拉设备列表 → 发起会话
- KEY 更换后所有请求返回 401，客户端重新配置即可

## 设备管理

设备注册表持久化于服务端数据目录（`devices.json`），会话与在线状态为内存状态，服务端重启后设备需重新心跳上线。

### `[P1] POST /api/v1/devices`

注册设备，返回 device_id。

```json
// 请求
{
  "name": "家里台式机",
  "platform": "windows",
  "fingerprint": "<sha256-hex，QUIC 自签名证书 DER 的哈希>",
  "library_name": "我的素材库",
  "allow_remote": true,
  "allow_write": false
}
// 响应
{ "status": "success", "data": { "device_id": "d_42" } }
```

`allow_remote`：设备级开关，关闭时该设备的会话请求被直接置 `rejected`（UI 里可改）。
`allow_write`：目标设备当前是否允许广域网写会话（对应 `[remote].writable`）；由设备注册/心跳上报，服务端用于设备列表展示与写会话预检，最终仍由目标 daemon 每请求裁决。

### `[P1] POST /api/v1/devices/heartbeat`

设备在线心跳，携带可达性信息。间隔 30s；服务端 90s 未收到心跳判离线。

```json
// 请求（access_key 鉴权；device_id 标识本机）
{
  "device_id": "d_42",
  "allow_write": false,
  "reachability": {
    "public_ip": "1.2.3.4",          // 设备自报公网 IPv4；服务端与连接源地址比对，以服务端为准（自报值可不填）
    "mapped_port": 27373,             // UPnP/NAT-PMP 映射成功的 QUIC UDP 端口；无映射为 null
    "mapping_type": "upnp",           // none / upnp / pmp
    "public_ipv6": "240e:xxxx::1",    // 全局 IPv6（通往服务端出接口的当前地址，临时地址轮换由心跳周期自然刷新）；无则 null
    "ipv6_port": 27373,               // QUIC 监听端口；IPv6 无 NAT 映射，防火墙放行即可入站，服务端无法验证可达性——B 的连接尝试即最终校验
    "nat_type": "cone"                // [P3 预留] 打洞用；服务端当前忽略
  }
}
// 响应
{ "status": "success", "data": {
    "server_time_ms": 1750000000000,
    "public_ip_seen": "1.2.3.4"       // 服务端看到的连接源地址（权威值）
} }
```

### `[P1] GET /api/v1/devices`

设备列表（access_key 鉴权）。`online` 以心跳为准（90s 内心跳为在线）；`reachability` 为最近一次心跳的上报（`public_ip` 为服务端权威值），未心跳过为 null：

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
        "fingerprint": "...",
        "allow_remote": true,
        "allow_write": false,
        "reachability": {
          "public_ip": "1.2.3.4",
          "mapped_port": 27373,
          "mapping_type": "upnp",
          "public_ipv6": "240e:xxxx::1",
          "ipv6_port": 27373
        }
      }
    ]
  }
}
```

### `[P1] PATCH /api/v1/devices/{device_id}`

更新设备（access_key 鉴权）：

```json
// 请求（字段均可选）
{
  "name": "新名字",
  "allow_remote": false,
  "library_name": "工作库",
  "fingerprint": "<新证书指纹>"
}
// 响应：更新后的设备信息（同 GET /devices 单条结构）
```

`fingerprint` 更新即证书轮换：新指纹立即生效，旧证书的会话随即失效。

### `[P1] DELETE /api/v1/devices/{device_id}`

注销设备（access_key 鉴权）。其进行中的会话一并取消，对端经 WSS 收到 `session.cancelled`。

## 控制通道（WSS）

设备建立 WebSocket 长连接接收服务端推送（会话请求、状态变更）。连接鉴权走查询参数：

```text
wss://<base>/api/v1/ws?key=<access_key>&device_id=<device_id>
```

消息统一信封：`{ "type": "...", ... }`。同一设备的新连接顶替旧连接（旧连接收到 Close）。

### 服务端 → 设备

| type                | 内容                                                                                   | 阶段 |
| ------------------- | -------------------------------------------------------------------------------------- | ---- |
| `hello`             | `{ "device_id", "server_time_ms" }`                                                    | P1   |
| `ping`              | `{}`（保活，30s 间隔，两次未回 pong 断开）                                              | P1   |
| `session.request`   | `{ "session_id", "from": { "device_id", "name", "library_name" }, "mode": "direct", "access": "read" \| "write" }` | P1   |
| `session.update`    | `{ "session_id", "state", ... }`（状态变更推送双方；轮询为兜底）                         | P1   |
| `session.cancelled` | `{ "session_id" }`                                                                     | P1   |

### 设备 → 服务端

| type        | 内容 | 说明                 |
| ----------- | ---- | -------------------- |
| `hello_ack` | `{}` | 应答 hello，连接就绪 |
| `pong`      | `{}` | 保活应答             |

业务动作（心跳、会话响应）一律走 REST，通道保持单向简单。

## 会话编排

### `[P1] POST /api/v1/sessions`

发起会话（access_key 鉴权）。`mode` 为 `direct`（本节）或 `relay`（见「中继」一节）。控制面会话默认 `access=read`；查看面写会话由 hawk-server 以 `access=write` 发起，要求目标设备 `allow_write=true`。

```json
// 请求
{ "source_device_id": "d_77", "target_device_id": "d_42", "mode": "direct" }
// 响应
{ "status": "success", "data": { "session_id": "s_9", "state": "pending" } }
```

服务端流程：

1. 校验来源/目标设备存在、目标设备在线（不在线返回 404）；`allow_remote=false` 直接置会话状态 `rejected`；`access=write` 且目标 `allow_write=false` 返回 403
2. 经 WSS 向目标推送 `session.request`（含 `access`）
3. 目标回 `POST /api/v1/sessions/{id}/accept`（access_key 鉴权），响应 `{ "status": "success" }`；拒绝则回 reject，会话状态变 `rejected`
4. 服务端取目标最近心跳的可达性组装候选列表（IPv4 映射优先、IPv6 次之；无任何候选则置 unreachable），经 WSS `session.update` 推送双方

发起方以 WSS 推送为准（轮询 1s 间隔、上限 30s 为兜底；超时状态变 `expired`）：

### `[P1] GET /api/v1/sessions/{session_id}`

会话状态即全部结果语义：

```json
// pending
{ "status": "success", "data": { "session_id": "s_9", "state": "pending" } }
// ready
{ "status": "success", "data": {
    "session_id": "s_9", "state": "ready",
    "connect": {
      "mode": "direct",
      "candidates": [
        { "transport": "ipv4", "host": "1.2.3.4", "port": 27373 },     // UPnP/NAT-PMP 映射候选
        { "transport": "ipv6", "host": "240e:xxxx::1", "port": 27373 } // 全局 IPv6 候选（无映射依赖）
      ],
      "fingerprint": "<sha256-hex，TOFU 钉扎目标 QUIC 证书>"
    }
} }
```

候选列表由服务端按心跳上报组装（IPv4 映射在前、IPv6 在后，缺失项省略）；客户端依序尝试，单候选超时 5s，任一成功即建立 QUIC 连接（指纹钉扎对所有候选一致）。**全部候选失败不是会话状态**——那是客户端本地判定直连失败：中继开关开启时弹窗询问用户是否经中继继续，未开启则提示到设置中开启；会话本身保持 ready，由用户选择后取消或另建中继会话。

| state         | 含义                                          | 客户端行为                                                 |
| ------------- | --------------------------------------------- | ---------------------------------------------------------- |
| `pending`     | 等待目标设备响应                              | 继续等待                                                   |
| `ready`       | 连接信息已就绪                                | 建立 QUIC 隧道                                             |
| `rejected`    | 目标设备拒绝（或 `allow_remote=false`）       | 展示"对方拒绝"                                             |
| `unreachable` | 目标无任何直连候选（无 v4 映射且无全局 IPv6） | 中继开关开启时询问用户是否走中继；未开启则提示到设置中开启 |
| `expired`     | 建立超时（30s）                               | 提供重试                                                   |
| `cancelled`   | 任一方取消（或设备注销）                      | 结束                                                       |

查看方本地代理（端口、会话随机 token）由发起方客户端自行生成，不进协议——隧道建立后由客户端拼装 web 查看器 URL（见数据面一节）。

### `[P1] POST /api/v1/sessions/{session_id}/accept`

目标设备接受会话（access_key 鉴权），请求体携带 `{ "device_id": "d_42" }` 标识本机，响应 `{ "status": "success" }`。仅 pending 状态可响应（重复响应或响应终态会话返回 409）；仅目标设备可响应（来源设备响应返回 403）。

### `[P1] POST /api/v1/sessions/{session_id}/reject`

目标设备拒绝会话（access_key 鉴权），请求体携带 `{ "device_id": "d_42" }` 标识本机；会话状态变 `rejected`。

### `[P1] POST /api/v1/sessions/{session_id}/cancel`

任一方取消会话（无请求体）。pending/ready → `cancelled`，双方经 WSS 收到 `session.cancelled`；已 cancelled 幂等成功；其余终态返回 409。

## 中继（P2）

直连失败时的兜底：会话数据面经服务端 WebSocket 字节转发。QUIC 仍在 A↔B 之间端到端建立（中继只搬运加密数据报，看不到素材内容）；单用户方案无票据/配额/用量上报，准入即控制面同一把 access key。

### `[P2] POST /api/v1/sessions`（mode=relay）

```json
// 请求
{ "source_device_id": "d_77", "target_device_id": "d_42", "mode": "relay" }
// 响应
{ "status": "success", "data": { "session_id": "s_9", "state": "pending" } }
```

流程同直连：推送 `session.request`（`mode` 为 `relay`）→ 目标 accept → 会话 ready。relay 模式的 `connect` 不携带候选（无需可达性上报）：

```json
{ "status": "success", "data": {
    "session_id": "s_9", "state": "ready",
    "connect": { "mode": "relay", "fingerprint": "<sha256-hex，TOFU 钉扎目标 QUIC 证书>" }
} }
```

### `[P2] GET /api/v1/relay`（WS 数据通道）

双方各自建立一条中继 WebSocket，服务端配对后逐帧转发：

```text
wss://<base>/api/v1/relay?key=<access_key>&device_id=<device_id>&session_id=<session_id>&role=a|b
```

- `role=a` 为目标设备（素材库所在机），`role=b` 为发起方；服务端校验：会话存在且 `state=ready`、`mode=relay`、device_id 与该会话对应角色一致（不符 403/404）
- **B 侧可以是服务端自身**：官方 hawk-server 的网页版宿主即以「网页版」设备身份（`platform="server"`）作为发起方走本流程——进程内中继 socket 取代真实 WS，协议字节零差异。浏览器与服务器之间的查看面 API（登录/设备/会话/代理）是服务端实现细节，不进本协议
- 双方到齐后逐条转发二进制帧（**一帧 = 一个 QUIC 数据报**），不解析内容；文本帧忽略
- 任一方断开 → 双方连接关闭，会话置 `cancelled`（双方控制通道收到 `session.cancelled`）
- 客户端行为约定：直连失败（候选全部超时或无候选/unreachable）且「中继回退」开启时，经用户确认后以 `mode=relay` 另建会话；任何环节都不自动回落

## 数据面（QUIC 隧道）

- 双方 QUIC 连接（quinn），自签名证书，对端身份 = 信令交换的指纹钉扎（TOFU）。证书轮换走 `PATCH /devices/{id}` 更新指纹：新指纹立即生效，旧证书会话失效
- 隧道内 HTTP 请求必须携带会话标识头 `x-hawk-session: <session_id>`（B 侧代理写入，客户端自带的同名头一律覆盖）。A 侧校验会话已接受、且与该 QUIC 连接的对端指纹一致后，在进程内插入 `TunneledPeer { writable }`；HTTP 客户端无法伪造该标记
- A 侧按会话权限转发到 `127.0.0.1:27371`：
  - 只读会话：viewer 级，仅 GET 与查询类 POST
  - 写会话：`access=write`，且请求时 `[remote].writable=true` 才生效；仅放行 GET、查询类 POST、`POST /api/v1/item/add`（禁止 `path`）与 `POST /api/v1/folder/create`，其余写端点 403
- **隧道端必须拒绝转发 `GET /api/v1/app/token`（直接返回 404）**。该端点免鉴权返回主 token（完全读写权限），仅服务于本机浏览器插件的零配置发现（插件直连环回，不经隧道）；远程查看方不需要它——B 侧已持有代理会话 token
- B 侧本地代理：监听 `127.0.0.1:27374`，每次会话随机 token，web 查看器以 `?token=` 携带；代理响应不带 CORS 头。代理端口由 Electron 预选后经参数下发（见 remote-access.md 进程接线），会话 token 由 B 侧 hawk-remote 生成，web 查看器 URL 由客户端拼装，不经过服务端
- 同一设备指纹可同时承载多个查看会话（例如 hawk-server 的网页版与插件）：A 侧指纹放行按会话引用计数，连接与会话绑定，取消一个会话只关闭该会话的连接与请求
- 隧道默认端口：A 侧 QUIC UDP 27373（UPnP 映射目标；双栈监听，IPv4/IPv6 同端口）；均可配置

## 默认值与时限

| 项                  | 值                                                                                              |
| ------------------- | ----------------------------------------------------------------------------------------------- |
| 心跳间隔 / 离线判定 | 30s / 90s                                                                                       |
| 心跳最小间隔        | 10s（429）                                                                                      |
| WSS 保活            | ping 30s 间隔，两次未回 pong 断开                                                               |
| 会话建立超时        | 30s                                                                                             |
| 直连单候选超时      | 5s（客户端实现，全部候选失败即直连失败）                                                        |
| 端口默认值          | hawk-server 27370 / hawk API 27371 / LAN web 27372 / 远程 QUIC UDP 27373 / 本地代理 27374 / remote 状态 API 27375（本地端口均由 Electron 预选后经参数下发） |

## 版本策略

- 协议版本隐含于路径 `/api/v1`；不兼容变更升 v2 并保留 v1 过渡期
- 新增可选字段向后兼容：客户端忽略未知字段，服务端对缺失字段取默认值
- WSS 消息同理：未知 `type` 忽略

# 远程访问协议 V1（remote-protocol）

> hawk 客户端（hawk-daemon 的 remote 模块，见 [remote-access.md](remote-access.md)）与远程访问服务端（hawk-server，本仓库 `hawk-server/`）之间的接口契约。协议是两侧的唯一共享物，实现零代码共享。
> 多租户方案：一台 hawk-server 可服务多个池（租户）。两级凭证——**service-key**（池门禁，程序生成，server 侧 `tenant add` 创建）与 **hawk-app key**（设备本地签发，逐把读/写权限，分发给网页版/插件等查看者）。协议无账号/会员/计费概念；账号系统后续接入时只替换 service-key 的签发水源，协议不动。访问被拒的语义为凭证无效（401）。
> 总体架构见 [remote-access.md](remote-access.md)。标注 `[P1]` 为直连控制面（已实现）；`[P2]` 为中继（已实现，见「中继」一节）；`[P3]` 为 NAT 打洞（已实现，见「NAT 打洞」一节）。

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
- 会话执法在目标 daemon：查看者会话的 `session.request` 携带 `credential`（s），daemon 比对本地 key 映射裁决权限——server 被攻破也无法把只读 key 放大成写权限
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
- 控制面全量 TLS：由前置反代（Caddy 等）终结，hawk-server 进程监听明文 HTTP
- 限速：心跳最小间隔 10s（违反返回 429）；凭证验证面限速见「凭证模型」

## 接入配置（协议视角）

**hawk-app 的最小知识 = 一个 URL + 一把 service-key；查看者的最小知识 = URL + service-key + 一把 hawk-app key。**

- 部署者用 `hawk-server tenant add --label <名>` 创建租户并拿到 service-key（明文只显示一次），分发给自己各台设备的 hawk-app；`tenant list` / `tenant revoke` 管理（吊销即该池 service-key 与池内 hawk-app key 映射全部失效）
- 客户端接入流程：`POST /devices` 注册设备（拿 device_id）→ 持久化 → `PUT /devices/{id}/keys` 同步 key 映射 → 心跳保活 → `GET /devices` 拉本池设备列表 → 发起会话
- service-key 吊销/更换后所有请求返回 401，客户端重新配置即可

## 设备管理

设备注册表持久化于服务端数据目录（`devices.json`，含 `tenant_id` 池归属），会话与在线状态为内存状态，服务端重启后设备需重新心跳上线。

### `[P1] POST /api/v1/devices`

注册设备（service-key 鉴权，设备入调用者所在池），返回 device_id。

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
`allow_write`：目标设备当前是否允许广域网写会话（对应应用级 `remote.toml` 的 `writable`）；由设备注册/心跳上报，服务端用于写会话预检，最终由目标 daemon 按本地 key 映射每请求裁决。
`library_name`：设备当前打开的素材库名。设备身份跨库共享，同一台设备切换库后经心跳更新设备表，避免列表展示旧库名。

### `[P1] POST /api/v1/devices/heartbeat`

设备在线心跳，携带可达性信息。间隔 30s；服务端 90s 未收到心跳判离线。设备须属调用者池（他池设备 404）。

```json
// 请求（service-key 鉴权；device_id 标识本机）
{
  "device_id": "d_42",
  "allow_write": false,
  "library_name": "我的素材库",      // 当前打开的库名；服务端据此收敛设备表（可省略）
  "reachability": {
    "public_ip": "1.2.3.4",          // 设备自报公网 IPv4；服务端与连接源地址比对，以服务端为准（自报值可不填）
    "mapped_port": 27373,             // UPnP/NAT-PMP 映射的外部端口（期望端口被占时为网关实际分配的任意端口）；无映射为 null
    "mapping_type": "upnp",           // none / upnp / pmp
    "public_ipv6": "240e:xxxx::1",    // 全局 IPv6（通往服务端出接口的当前地址，临时地址轮换由心跳周期自然刷新）；无则 null
    "ipv6_port": 27373,               // QUIC 监听端口；IPv6 无 NAT 映射，防火墙放行即可入站，服务端无法验证可达性——B 的连接尝试即最终校验
    "lan_ipv4": "192.168.1.5",        // LAN IPv4（出接口私网地址）：双方同公网 IP 时作 LAN 候选下发；非公网私网地址不上报
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

本池设备列表（service-key 鉴权；不含「网页版」全局设备）。`online` 以心跳为准（90s 内心跳为在线）；`reachability` 为最近一次心跳的上报（`public_ip` 为服务端权威值），未心跳过为 null：

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

更新设备（service-key 鉴权；他池设备 404）：

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

注销设备（service-key 鉴权；他池设备 404）。其进行中的会话一并取消，对端经 WSS 收到 `session.cancelled`；该设备的 hawk-app key 映射一并清除。

### `[P1] PUT /api/v1/devices/{device_id}/keys`

全量同步本设备的 hawk-app key 映射（service-key 鉴权；覆盖式，daemon 在 key 变更与每次接入时推送）。daemon 只上送验证子 `s = sha256(key)`，server 不见明文：

```json
// 请求
{ "keys": [ { "s": "<sha256-hex>", "read": true, "write": false } ] }
// 响应
{ "status": "success", "data": { "synced": 1 } }
```

校验：`s` 须 64 位小写十六进制；`read`/`write` 至少一项；任一条目非法整体拒绝（400）。

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
{ "status": "success", "data": { "token": "hb_...", "access": "write", "device_id": "d_42" } }
```

key 无效、已吊销、或不属于门令牌绑定的池，同按 401「访问密钥无效」处理（不泄露存在性）。

## 控制通道（WSS）

设备建立 WebSocket 长连接接收服务端推送（会话请求、状态变更）。鉴权走 `Authorization: Bearer <service_key>` 头（**不走 URL 查询串**）：

```text
wss://<base>/api/v1/ws?device_id=<device_id>
```

消息统一信封：`{ "type": "...", ... }`。同一设备的新连接顶替旧连接（旧连接收到 Close）。

### 服务端 → 设备

| type                | 内容                                                                                   | 阶段 |
| ------------------- | -------------------------------------------------------------------------------------- | ---- |
| `hello`             | `{ "device_id", "server_time_ms" }`                                                    | P1   |
| `ping`              | `{}`（保活，30s 间隔，两次未回 pong 断开）                                              | P1   |
| `session.request`   | `{ "session_id", "from": { "device_id", "name", "library_name", "fingerprint" }, "mode": "direct", "access": "read" \| "write", "credential": "<sha256-hex 或 null>" }` | P1   |
| `session.update`    | `{ "session_id", "state", ... }`（状态变更推送双方；轮询为兜底）                         | P1   |
| `session.probe`     | `{ "session_id", "probe_port", "token": "<32-hex>" }`（打洞探测邀约，见「NAT 打洞」一节） | P3   |
| `session.cancelled` | `{ "session_id" }`                                                                     | P1   |

`from.fingerprint`：发起方的 QUIC 证书指纹（服务端持有双方设备记录，随请求下发，省一次设备列表往返）。`credential`：查看者会话的 key 验证子（app 间会话为 null）；目标 daemon 据此比对本地 key 映射执法——未命中/权限不足/本机 writable 关闭即拒绝。

### 设备 → 服务端

| type        | 内容 | 说明                 |
| ----------- | ---- | -------------------- |
| `hello_ack` | `{}` | 应答 hello，连接就绪 |
| `pong`      | `{}` | 保活应答             |

业务动作（心跳、会话响应）一律走 REST，通道保持单向简单。

客户端以服务端帧为通道活性信号：超过 90s（3 个 ping 周期）未收到任何服务端帧即判定连接半开，主动断开重连。中间盒静默断链（反代切换、NAT 会话老化）时上行 REST 心跳可能仍然通畅——不能以上行心跳自证在线，否则设备会永远「假在线」而收不到任何推送。

## 会话编排

### `[P1] POST /api/v1/sessions`

发起 app 间会话（service-key 鉴权；双方设备须同池，跨池 404）。`mode` 为 `direct`（本节）或 `relay`（见「中继」一节）。app 间会话默认 `access=read`、不携带凭证；查看面会话由 hawk-server 以「网页版」全局设备身份发起（relay 模式），携带查看者凭证，`access=write` 要求目标设备 `allow_write=true`。

```json
// 请求
{ "source_device_id": "d_77", "target_device_id": "d_42", "mode": "direct" }
// 响应
{ "status": "success", "data": { "session_id": "s_9", "state": "pending" } }
```

服务端流程：

1. 校验来源/目标设备存在且同池、目标设备在线（不在线返回 404）；`allow_remote=false` 直接置会话状态 `rejected`；`access=write` 且目标 `allow_write=false` 返回 403
2. 经 WSS 向目标推送 `session.request`（含 `access`、查看者会话含 `credential`）
3. 目标回 `POST /api/v1/sessions/{id}/accept`（service-key 鉴权），响应 `{ "status": "success" }`；拒绝则回 reject，会话状态变 `rejected`。目标 daemon 对查看者会话做本地执法（key 存在性/权限/writable 总开关），不通过即 reject
4. 服务端组装连接信息：探测口启用时直连会话一律进入打洞编排（见「NAT 打洞」一节）——无直连候选等探测到齐（8s），有候选短等附带 punch 候选（2s）；探测口未启用则取目标最近心跳组装候选立即 ready（无任何候选置 unreachable），经 WSS `session.update` 推送双方

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
      "fingerprint": "<sha256-hex，TOFU 钉扎目标 QUIC 证书>",
      "punch_peer": { "host": "5.6.7.8", "port": 40123 }              // [P3] 仅打洞编排时存在：发起方的 observed endpoint，目标设备开口用（发起方忽略）
    }
} }
```

候选列表由服务端按心跳上报组装，顺序：**LAN 候选**（双方同公网 IP 且目标上报 `lan_ipv4` 时，`{ "transport": "lan", ... }` 排最前——同局域网内网直连，绕开 hairpin/UPnP/CGNAT；错配由指纹钉扎快速证伪）→ IPv4 映射 → IPv6 → 打洞候选 `{ "transport": "punch", ... }` 排最后（见「NAT 打洞」一节），缺失项省略。客户端对全部候选**并行竞速**（Happy Eyeballs：首个握手成功者赢，败者即取消），直连候选单候选超时 5s（punch 候选 15s），任一成功即建立 QUIC 连接（指纹钉扎对所有候选一致）。**全部候选失败不是会话状态**——那是客户端本地判定直连失败：中继开关开启时弹窗询问用户是否经中继继续，未开启则提示到设置中开启；会话本身保持 ready，由用户选择后取消或另建中继会话。

| state         | 含义                                          | 客户端行为                                                 |
| ------------- | --------------------------------------------- | ---------------------------------------------------------- |
| `pending`     | 等待目标设备响应                              | 继续等待                                                   |
| `ready`       | 连接信息已就绪                                | 建立 QUIC 隧道                                             |
| `rejected`    | 目标设备拒绝（或 `allow_remote=false`，或查看者 key 未过本地执法） | 展示"对方拒绝"                                             |
| `unreachable` | 目标无任何可用直连路径（无 v4 映射、无全局 IPv6，且打洞编排不可用或探测失败） | 中继开关开启时询问用户是否走中继；未开启则提示到设置中开启 |
| `expired`     | 建立超时（30s）                               | 提供重试                                                   |
| `cancelled`   | 任一方取消（或设备注销）                      | 结束                                                       |

查看方本地代理（端口、会话随机 token）由发起方客户端自行生成，不进协议——隧道建立后由客户端拼装 web 查看器 URL（见数据面一节）。

### `[P1] POST /api/v1/sessions/{session_id}/accept`

目标设备接受会话（service-key 鉴权），请求体携带 `{ "device_id": "d_42" }` 标识本机，响应 `{ "status": "success" }`。仅 pending 状态可响应（重复响应或响应终态会话返回 409）；仅目标设备可响应（来源设备响应返回 403）；目标设备须属调用者池（跨池 404）。

### `[P1] POST /api/v1/sessions/{session_id}/reject`

目标设备拒绝会话（service-key 鉴权），请求体携带 `{ "device_id": "d_42" }` 标识本机；会话状态变 `rejected`。

### `[P1] POST /api/v1/sessions/{session_id}/cancel`

任一方取消会话（无请求体；会话双方之一须属调用者池）。pending/ready → `cancelled`，双方经 WSS 收到 `session.cancelled`；已 cancelled 幂等成功；其余终态返回 409。

## 中继（P2）

直连失败时的兜底：会话数据面经服务端 WebSocket 字节转发。QUIC 仍在 A↔B 之间端到端建立（中继只搬运加密数据报，看不到素材内容）；多租户方案无票据/配额/用量上报，准入即本池 service-key。

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

双方各自建立一条中继 WebSocket（鉴权走 `Authorization: Bearer <service_key>` 头），服务端配对后逐帧转发：

```text
wss://<base>/api/v1/relay?device_id=<device_id>&session_id=<session_id>&role=a|b
```

- `role=a` 为目标设备（素材库所在机），`role=b` 为发起方；服务端校验：会话存在且 `state=ready`、`mode=relay`、device_id 与该会话对应角色一致（不符 403/404）、设备属调用者池
- **B 侧可以是服务端自身**：官方 hawk-server 的网页版宿主即以「网页版」全局设备身份作为发起方走本流程——进程内中继 socket 取代真实 WS，协议字节零差异。浏览器与服务器之间的查看面登录是服务端实现细节（见「查看面登录」）
- 双方到齐后逐条转发二进制帧（**一帧 = 一个 QUIC 数据报**），不解析内容；文本帧忽略
- 任一方断开 → 双方连接关闭，会话置 `cancelled`（双方控制通道收到 `session.cancelled`）
- 客户端行为约定：直连失败（候选全部超时或无候选/unreachable）且「中继回退」开启时，经用户确认后以 `mode=relay` 另建会话；任何环节都不自动回落

## NAT 打洞（P3）

目标无可用直连路径时的直连补救与兑底：服务端撮合双方「被观测到的公网 endpoint」（observed endpoint），双方同时开口打通各自 NAT，QUIC 仍端到端建立（指纹钉扎语义不变——打洞只决定包能否到达）。

**为什么有直连候选也要编排打洞**：UPnP 映射可能开在 CGNAT 内层、全局 IPv6 可能被入站防火墙拦截——服务端无法验证候选的真实可用性。因此探测口启用时直连会话一律编排：无直连候选时打洞是唯一指望（等探测到齐，8s 上限）；有候选时短等（2s）把 punch 候选附在队尾兑底，不显著拖累可用路径。

**为什么不走心跳上报**：UDP NAT 映射生命周期短（常见 30~120s），且发起方的 QUIC socket 是会话时才建的——observed endpoint 必须在会话建立时新鲜探测，心跳周期上报的值不可信。

### 打洞探测口（服务端 UDP，默认 9101）

极简 STUN 式应答，只服务「观测会话参与者的公网 endpoint」一件事：

```text
请求（定长 64 字节）："HAWKP"（5B magic）+ token（16B）+ 零填充
响应（≤64 字节）：   "HAWKR" + token + 地址族（1B：4/6）+ IP（4B/16B）+ 端口（2B，大端）
否定应答（21 字节）： "HAWK?" + token（token 无效或编排已结束；不大于请求，无放大风险）
```

- token 由服务端按会话按角色签发（经 WSS `session.probe` 下发，32 位小写 hex），是探测包归属会话的唯一依据；无效 token 回**否定应答**——运维可据此区分「探测口不可达」与「token 错误」（tools/check-probe-port.mjs 即以此原理做连通性检查）；形状非法（不足 64B / magic 不符）与超限流量的包才静默丢弃
- 响应不大于请求 + 按来源 IP 限速（10s 窗口 30 包，超限丢弃），杜绝反射放大滥用
- 回包仅作诊断（客户端可比对本地端口感知 NAT 存在性），客户端实现可以不消费
- 探测口仅绑 IPv4：打洞发生在 v4 NAT 域；v6 无 NAT，可达性走心跳上报的直连候选
- 部署：探测口需公网直达（--probe-port，0 = 关闭打洞编排）；反代只暴露 443 的部署探测不到，退化为「无直连候选即 unreachable」，不破坏既有行为

### 编排时序

```text
B POST /sessions(direct) → A accept → 探测口已启用：会话保持 pending，
  服务端向双方各推 session.probe（token 各自独立）
  → 双方各自用「自己 QUIC 端点的同一个 UDP socket」向 <服务端 host>:<probe_port>
    发探测包（3 次，间隔 300ms，容忍单包丢失；host 即接入配置 base URL 的主机名）
  → 服务端记录双方 observed endpoint（无直连候选等 8s，有候选短等 2s）
  → 到齐：会话 ready，经 WSS session.update 推送双方——
       connect.candidates = [...直连候选, { "transport": "punch", host, port }]  // punch = A 的 observed endpoint，排最后
       connect.punch_peer  = { host, port }                                     // B 的 observed endpoint，仅 A 使用
  → 未到齐且无直连候选：会话置 unreachable；未到齐但有候选：按候选 ready（无 punch 字段）
```

探测口不可达、token 丢失、UDP 被运营商拦截等失败形态统一收敛为「超时 → 退化路径」（无候选即 unreachable，有候选按候选走），客户端无需区分。

### 开口与连接（客户端行为）

- **A（目标设备）**：收到 ready 且 `connect.punch_peer` 存在 → 用 QUIC 端点同端口 socket 每 500ms 向 punch_peer 发开口包（16 字节垃圾数据；对端 quinn 收到无法解析的数据报自然丢弃），窗口 20s
- **B（发起方）**：ready 的 candidates 含 punch 候选 → 用「探测时同一个 socket」建 QUIC endpoint 发起 connect（单候选超时 15s 而非 5s：开口依赖双方时序，QUIC Initial 自动重传覆盖洞口开启前的丢包）。探测与连接必须同端口——cone NAT 的映射按本地端口建立
- 不需要时钟同步：双方以「收到 ready 即开打」为准，重传与窗口长度吸收时序差
- 对称型 NAT（任一侧）观测端口随目的地址变化，打洞必然失败 → 15s 候选超时 → 按既有「直连失败」处理（中继回退开关开启时询问用户）
- 心跳 `nat_type` 字段继续保留：当前客户端一律报 `unknown`，服务端不据此决策

## 数据面（QUIC 隧道）

- 双方 QUIC 连接（quinn），自签名证书，对端身份 = 信令交换的指纹钉扎（TOFU）。证书轮换走 `PATCH /devices/{id}` 更新指纹：新指纹立即生效，旧证书会话失效
- 隧道内 HTTP 请求必须携带会话标识头 `x-hawk-session: <session_id>`（B 侧代理写入，客户端自带的同名头一律覆盖）。A 侧校验会话已接受、且与该 QUIC 连接的对端指纹一致后，在进程内插入 `TunneledPeer { writable }`；HTTP 客户端无法伪造该标记
- A 侧按会话权限转发到本机 API：
  - 只读会话：viewer 级，仅 GET 与查询类 POST
  - 写会话：`access=write` 且请求时 `remote.writable=true` 且查看者 key 带写权限（本地执法三重裁决）；仅放行 GET、查询类 POST、`POST /api/v1/item/add`（禁止 `path`）与 `POST /api/v1/folder/create`，其余写端点 403
- **隧道端必须拒绝转发 `GET /api/v1/app/token`（直接返回 404）**。该端点免鉴权返回主 token（完全读写权限），仅服务于本机浏览器插件的零配置发现（插件直连环回，不经隧道）；远程查看方不需要它——B 侧已持有代理会话 token
- B 侧本地代理：监听环回随机端口，每次会话随机 token，web 查看器以 `?token=` 携带；代理响应不带 CORS 头。代理端口由客户端会话建立时分配，会话 token 由 B 侧生成，web 查看器 URL 由客户端拼装，不经过服务端
- 同一设备指纹可同时承载多个查看会话（例如 hawk-server 的网页版与插件）：A 侧指纹放行按会话引用计数，连接与会话绑定，取消一个会话只关闭该会话的连接与请求
- 隧道默认端口：A 侧 QUIC UDP 27373（UPnP 映射目标；双栈监听，IPv4/IPv6 同端口）；均可配置

## 默认值与时限

| 项                    | 值                                                                                              |
| --------------------- | ----------------------------------------------------------------------------------------------- |
| 心跳间隔 / 离线判定   | 30s / 90s                                                                                       |
| 心跳最小间隔          | 10s（429）                                                                                      |
| WSS 保活              | ping 30s 间隔，两次未回 pong 断开；客户端 90s 未收到服务端帧即主动断开重连                       |
| 会话建立超时          | 30s                                                                                             |
| 直连单候选超时        | 5s（客户端实现；punch 候选 15s；全部候选并行竞速，全失败即直连失败）                                        |
| 打洞编排等待上限      | 无直连候选 8s（超时置 unreachable）/ 有候选 2s（超时按候选放行）                                  |
| 开口包间隔 / 窗口     | 500ms / 20s（A 侧客户端实现）                                                                   |
| 探测包重发            | 3 次 × 300ms（客户端实现，容忍单包丢失）                                                        |
| 探测口限速            | 单来源 IP 10s 窗口 30 包（超限静默丢弃）                                                        |
| 门令牌 TTL / 尝试预算 | 5min / 5 次                                                                                     |
| 风控                  | 单 IP 10min 内 30 次失败 → 冷却 15min；单租户 20 次登录失败 → 池冷却 15min 起指数增长（封顶 24h），成功清零 |
| 端口默认值            | hawk-server 9100 / 打洞探测口 UDP 9101 / hawk API 27371 / LAN web 27372 / 远程 QUIC UDP 27373（本地端口由客户端在会话建立时分配） |

## 版本策略

- 协议版本隐含于路径 `/api/v1`；不兼容变更升 v2 并保留 v1 过渡期
- 新增可选字段向后兼容：客户端忽略未知字段，服务端对缺失字段取默认值
- WSS 消息同理：未知 `type` 忽略

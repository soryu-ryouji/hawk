# 远程访问协议 V1（remote-protocol）

> hawk 客户端（remote 模块）与云端服务之间的接口契约。本协议是唯一共享物：云端服务（hawk-remote-server，闭源）照此实现；hawk 仓库内不包含任何云端实现代码。
> 总体架构见 [remote-access.md](remote-access.md)。客户端不理解服务端的准入规则：协议无任何会员/计费字段，访问被拒以 HTTP 401 表达（message 说明原因）。
> 标注 `[P1]` 的为阶段 1 范围，`[P2]` 阶段 2，`[P3]` 阶段 3 占位。

## 通用约定

- 控制面前缀 `/api/v1`，官方端点地址由官方构建注入（文档以 `<base>` 占位）；自托管构建由 `--remote <base>` 指定
- 请求与响应字段 snake_case，时间戳 Unix 毫秒
- 响应信封沿用 hawk 本体约定，错误信封不带业务错误码——语义由 HTTP 状态码承载，`message` 供客户端直接展示：

```json
{ "status": "success", "data": {} }
{ "status": "error", "error": { "message": "访问被拒" } }
```

### HTTP 状态码（即全部错误语义）

| 状态码       | 含义                                                                                | 客户端行为       |
| ------------ | ----------------------------------------------------------------------------------- | ---------------- |
| 401          | 访问被拒（服务端自行定义拒绝规则，message 说明原因）                                | 展示 message     |
| 其他 4xx/5xx | 按 HTTP 惯例（400 参数、403 设备上限、404 不存在/不在线、429 限速、500 服务端错误） | 一律展示 message |

**客户端对所有错误的处理一致：直接展示服务端返回的 message。** message 是服务端生成的自由文本，需要引导用户（重新签发密钥、调整设备等）时服务端可在其中附带 URL；客户端不内置任何服务方的指引地址，用谁的服务就展示谁给出的指引。会话的失败结果（拒绝/超时/不可直连）不进 HTTP 错误——它们是会话状态（见会话编排）。

- 鉴权头：`Authorization: Bearer <access_key>`。**唯一凭据 access_key**：由账户服务签发/撤销（签发方式不在协议范围内），代表账号身份，所有端点统一使用。客户端配置 = 一个 URL（信令地址）+ 一个 KEY（访问密钥），无登录流程、无会话令牌
- **device_id**：设备注册时由服务端分配，客户端持久化，随请求携带（用于心跳、会话响应、WSS 等需要标识设备身份的场合）
- access_key 为 256-bit 随机值，服务端只存哈希
- 控制面全量 TLS
- 限速：心跳最小间隔 10s；其余端点按 access_key 限速由服务端自定

## 接入配置（协议视角）

**客户端的最小知识 = 一个 URL + 一个 KEY。** 协议中没有登录端点：

- access_key 由账户服务签发/撤销（自托管者自定）；官方构建内置服务端点地址，用户只需粘贴 KEY；自托管构建由 `--remote` 指定 URL 与 KEY
- 自托管者自行决定 KEY 的判定逻辑：固定密钥、自建密钥表、或任意放行
- 客户端接入流程：`POST /devices` 注册设备（拿 device_id）→ 持久化 → 心跳保活 → `GET /devices` 拉设备列表 → 发起会话
- KEY 撤销后所有请求返回 401；需要多设备隔离时可为每台设备签发独立 KEY

## 访问控制（协议视角）

**客户端不理解服务端的准入规则。** 协议只定义一件事：服务端可在任何端点以 401 拒绝访问（message 说明原因），客户端与其他错误一致，直接展示 message。需要引导用户操作时由服务端在 message 中附带指引或 URL，客户端不内置任何服务方的指引地址。

- 拒绝原因（密钥无效/已撤销、准入资格、额度等）是服务端内部规则，协议不枚举，客户端不区分
- 自托管者自行决定准入逻辑：固定密钥、自建密钥表、或任意放行

## 设备管理

设备数上限由服务端实现（注册时统计已注册设备数，超限返回 403 + message），**不进协议、客户端不需要知道上限数值**。超限不硬拒绝：客户端展示 message，用户可在设备列表中自行注销一台（`DELETE /devices/{id}`）后重试。上限调整只需改服务端代码，客户端无感。

### `[P1] POST /api/v1/devices`

注册设备，返回 device_id。服务端可在此拒绝访问（401）。

```json
// 请求
{
  "name": "家里台式机",
  "platform": "windows",
  "fingerprint": "<sha256-hex，QUIC 自签名证书 DER 的哈希>",
  "library_name": "我的素材库",
  "allow_remote": true
}
// 响应
{ "status": "success", "data": { "device_id": "d_42" } }
```

`allow_remote`：设备级开关，关闭时该设备的会话请求被自动拒绝（UI 里可改）。

### `[P1] POST /api/v1/devices/heartbeat`

设备在线心跳，携带可达性信息。间隔 30s；服务端 90s 未收到心跳判离线。

```json
// 请求（access_key 鉴权；device_id 标识本机）
{
  "device_id": "d_42",
  "reachability": {
    "public_ip": "1.2.3.4",          // 设备自报公网 IP；服务端与连接源地址比对，不一致时以服务端为准
    "mapped_port": 27373,             // UPnP/NAT-PMP 映射成功的 QUIC UDP 端口；无映射为 null
    "mapping_type": "upnp",           // none / upnp / pmp
    "nat_type": "cone"                // [P3] 打洞用；阶段 1 恒 unknown
  }
}
// 响应
{ "status": "success", "data": {
    "server_time_ms": 1750000000000,
    "public_ip_seen": "1.2.3.4"       // 服务端看到的连接源地址（权威值）
} }
```

### `[P1] GET /api/v1/devices`

账号下设备列表（access_key 鉴权）：

```json
{
  "status": "success",
  "data": {
    "devices": [
      {
        "device_id": "d_42",
        "name": "家里台式机",
        "library_name": "我的素材库",
        "online": true,
        "last_seen_at_ms": 1750000000000,
        "fingerprint": "...",
        "allow_remote": true,
        "reachability": {
          "public_ip": "1.2.3.4",
          "mapped_port": 27373,
          "mapping_type": "upnp"
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

注销设备（access_key 鉴权）。

## 控制通道（WSS）

设备建立 WebSocket 长连接接收服务端推送（会话请求、票据下发、打洞协调）。连接鉴权走查询参数：

```text
wss://<base>/api/v1/ws?key=<access_key>&device_id=<device_id>
```

消息统一信封：`{ "type": "...", ... }`。

### 服务端 → 设备

| type                   | 内容                                                                                                                 | 阶段 |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------- | ---- |
| `hello`                | `{ "device_id", "server_time_ms" }`                                                                                  | P1   |
| `ping`                 | `{}`（保活，30s 间隔，两次未回 pong 断开）                                                                           | P1   |
| `session.request`      | `{ "session_id", "from": { "device_id", "name", "library_name" }, "mode": "direct" }`                                | P1   |
| `session.update`       | `{ "session_id", "state", ... }`（状态变更推送双方；轮询为兜底）                                                     | P1   |
| `session.cancelled`    | `{ "session_id" }`                                                                                                   | P1   |
| `session.relay_ticket` | `{ "session_id", "ticket": "<base64>", "relay": { "host", "port", "transport": "udp" }, "peer_fingerprint": "..." }` | P2   |
| `punch.plan`           | `{ "session_id", "peer": { "public_ip", "port" }, "local_port", "deadline_ms" }`                                     | P3   |

### 设备 → 服务端

| type        | 内容 | 说明                 |
| ----------- | ---- | -------------------- |
| `hello_ack` | `{}` | 应答 hello，连接就绪 |
| `pong`      | `{}` | 保活应答             |

业务动作（心跳、会话响应）一律走 REST，通道保持单向简单。

## 会话编排

### `[P1] POST /api/v1/sessions`

发起会话（access_key 鉴权）。服务端可在此拒绝访问（401）。

```json
// 请求
{ "source_device_id": "d_77", "target_device_id": "d_42", "mode": "direct" }
// 响应
{ "status": "success", "data": { "session_id": "s_9", "state": "pending" } }
```

服务端流程：

1. 校验目标设备在线且访问资格有效（服务端规则）；`allow_remote=false` 直接置会话状态 `rejected`
2. 经 WSS 向目标推送 `session.request`
3. 目标校验来源后回 `POST /api/v1/sessions/{id}/accept`（access_key 鉴权），响应 `{ "status": "success" }`；拒绝则回 reject，会话状态变 `rejected`
4. 服务端取目标最近心跳的可达性组装连接信息，经 WSS `session.update` 推送双方

发起方以 WSS 推送为准（轮询 1s 间隔、上限 30s 为兜底；超时状态变 `expired`）：

### `[P1] GET /api/v1/sessions/{session_id}`

会话状态即全部结果语义：

```json
// pending
{ "status": "success", "data": { "session_id": "s_9", "state": "pending" } }
// ready（直连模式）
{ "status": "success", "data": {
    "session_id": "s_9", "state": "ready",
    "connect": {
      "mode": "direct",
      "public_ip": "1.2.3.4",
      "mapped_port": 27373,
      "fingerprint": "<sha256-hex，TOFU 钉扎目标 QUIC 证书>"
    }
} }
```

| state         | 含义                                    | 客户端行为                                      |
| ------------- | --------------------------------------- | ----------------------------------------------- |
| `pending`     | 等待目标设备响应                        | 继续等待                                        |
| `ready`       | 连接信息已就绪                          | 建立 QUIC 隧道                                  |
| `rejected`    | 目标设备拒绝（或 `allow_remote=false`） | 展示"对方拒绝"                                  |
| `unreachable` | 目标无公网可达路径                      | 提供"走中继"选项（`POST /sessions` mode=relay） |
| `expired`     | 建立超时（30s）                         | 提供重试                                        |
| `cancelled`   | 任一方取消                              | 结束                                            |

查看方本地代理（端口、会话随机 token）由发起方客户端自行生成，不进协议——隧道建立后由客户端拼装 web 查看器 URL（见数据面一节）。

### `[P1] POST /api/v1/sessions/{session_id}/accept`

目标设备接受会话（access_key 鉴权），请求体携带 `{ "device_id": "d_42" }` 标识本机，响应 `{ "status": "success" }`。

### `[P1] POST /api/v1/sessions/{session_id}/reject`

目标设备拒绝会话（access_key 鉴权），请求体携带 `{ "device_id": "d_42" }` 标识本机；会话状态变 `rejected`。

### `[P2] POST /api/v1/sessions`（mode=relay）

```json
// 请求
{ "source_device_id": "d_77", "target_device_id": "d_42", "mode": "relay" }
// 响应
{ "status": "success", "data": { "session_id": "s_9", "state": "pending" } }
```

服务端校验中继剩余额度（服务端规则，不足 → HTTP 401），按直连流程推送 `session.request`（mode=relay）给目标设备；目标接受后签发两张票据，经双方 WSS 下发 `session.relay_ticket`（见下节），会话进入 `ready`。

### `[P1] POST /api/v1/sessions/{session_id}/cancel`

任一方取消会话。

## 中继协议 `[P2]`

### 票据

账户服务签发给会话双方，HMAC-SHA256 签名（密钥为账户服务与中继节点间的共享密钥）：

```json
// ticket（base64 编码后下发）
{
  "v": 1,
  "session_id": "s_9",
  "role": "a", // 会话双方各一张：a / b
  "issued_at_ms": 1750000000000,
  "expires_at_ms": 1750000600000, // 建立窗口 10 分钟；建立后按会话上限（24h）续期
  "quota_bytes": 10737418240, // 本次会话字节配额（服务端分配，用于资源保护）
  "relay_host": "r1.example",
  "relay_port": 4433,
  "transport": "udp", // udp / tcp（TCP 帧模式，见下）
  "sig": "<hex>"
}
```

### 建立与转发

1. 双方以各自票据连接中继节点（UDP 数据报；transport=tcp 时为 TCP 帧模式）
2. 首个数据报为 hello：`{ "ticket": "<base64>" }`，中继验签后将会话绑定到来源地址
3. 两张票据（role=a/b）均已到达 → 配对完成，此后中继在双方源地址间**逐包原样转发**，不解析内容
4. 此后双方之间的 QUIC 握手与全部数据流都经此通道端到端建立——中继只搬运加密数据报

### TCP 帧模式

UDP 被网络封锁时使用。双方以 TCP 连接中继，数据以 2 字节大端长度前缀 + QUIC 数据报分帧传输，中继在两条 TCP 连接间转接帧内容。同样只转发不解析。

### 计量与断流

配额用于防止单会话过度消耗服务端带宽资源；额度数值与发放策略是服务端内部规则，协议不定义。

- 中继按会话累计转发字节数，周期（60s）与结束时经内网 POST `/internal/usage` 上报账户服务（共享密钥鉴权），账户服务按服务端规则记录用量
- 剩余额度不足 → 会话建立返回 401；已建立会话由中继断流，客户端下次建立时收到 401，直接展示 message
- 票据超期 → 断开；会话主动结束时双方关闭隧道，中继上报最终用量后回收会话

## 数据面（QUIC 隧道）

- 双方 QUIC 连接（quinn），自签名证书，对端身份 = 信令交换的指纹钉扎（TOFU）。证书轮换走 `PATCH /devices/{id}` 更新指纹：新指纹立即生效，旧证书会话失效
- A 侧隧道端：将隧道内收到的 HTTP 请求转发到 `127.0.0.1:27371`，注入 `Authorization: Bearer <远端只读 token>`（hawk-server 启动时经 env 注入的第三种 token，只读级别）
- B 侧本地代理：监听 `127.0.0.1:27374`，每次会话随机 token，web 查看器以 `?token=` 携带；代理响应不带 CORS 头。代理端口与 token 由 B 侧 remote 模块本地生成，web 查看器 URL 由客户端拼装，不经过信令服务
- 隧道默认端口：A 侧 QUIC UDP 27373（UPnP 映射目标）；均可配置

## 默认值与时限

| 项                  | 值                                                                    |
| ------------------- | --------------------------------------------------------------------- |
| 心跳间隔 / 离线判定 | 30s / 90s                                                             |
| WSS 保活            | ping 30s 间隔，两次未回 pong 断开                                     |
| 会话建立超时        | 30s                                                                   |
| 中继票据建立窗口    | 10 分钟                                                               |
| 中继会话时长上限    | 24 小时                                                               |
| 设备数上限          | 服务端逻辑，不进协议                                                  |
| 中继用量上报周期    | 60s                                                                   |
| 端口默认值          | hawk API 27371 / LAN web 27372 / 远程 QUIC UDP 27373 / 本地代理 27374 |

## 版本策略

- 协议版本隐含于路径 `/api/v1`；不兼容变更升 v2 并保留 v1 过渡期
- 新增可选字段向后兼容：客户端忽略未知字段，服务端对缺失字段取默认值
- WSS 消息同理：未知 `type` 忽略

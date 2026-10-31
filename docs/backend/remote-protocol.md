# 远程访问协议 V2（控制面）

> 本文描述当前已实现的 hawk-server 控制面契约。目标权限与全加密传输架构见 [access-control-v2.md](access-control-v2.md)。
> 公网网页版、ViewerHost、viewer/login、viewer token 和 server 侧 AccessKey 映射已删除。AccessKey-only rendezvous、原生 LAN/WAN 统一隧道和租约续期按实施计划推进，不在本文冒充已完成能力。

## 1. 基本约定

- 控制面前缀 `/api/v1`；请求与响应字段 snake_case，时间戳 Unix 毫秒。
- 成功响应：`{ "status": "success", "data": ... }`；无 data 时省略 data。
- 错误响应：`{ "status": "error", "error": { "message": "..." } }`。
- 控制面只处理 TenantAdmin、Device、设备在线状态和 Device 间会话编排，不接触素材数据或 AccessKey。
- 所有控制面请求必须使用 HTTPS/WSS；本地开发可使用明文 HTTP/WS。
- 除认证端点外，API 均使用 `Authorization: Bearer <session_token>`。
- Device 身份来自 Device Session，不接受请求体或查询字符串自报 device_id/source_device_id。

## 2. 凭证与 Session

### 2.1 tenant_key

`hawk-server tenant add` 生成 tenant_key（当前沿用 `hs_` 前缀和 256-bit 随机）。服务端只存 `HMAC-SHA256(pepper, tenant_key)`，明文只显示一次。

tenant_key 只用于换取 TenantAdmin Session，不能用于：

- 设备心跳、WSS 或 Device 间会话
- 素材读取
- AccessKey 查看者登录

### 2.2 TenantAdmin Session

默认有效期 2 小时，仅用于：

- 注册新设备
- 删除本租户设备

#### `POST /api/v1/tenant/session`

无需既有 Session；请求体携带 tenant_key。

```json
// 请求
{ "tenant_key": "hs_..." }

// 响应
{
  "status": "success",
  "data": {
    "session_token": "ts_...",
    "tenant_id": "t_...",
    "expires_in": 7200
  }
}
```

### 2.3 Device 身份与 Device Session

设备在本地生成 iroh/Ed25519 密钥对，endpoint_id 是公钥。注册后不再持有独立 device_token；每次需要控制面 Session 时，通过私钥签名短期 nonce 换取 Device Session。

Device Session 默认有效期 24 小时。服务端只存 Session 的 SHA-256 验证子，明文只由认证响应返回。daemon 在 Session 过期或失效后自动重新签名认证。

#### 签名消息

```text
"hawk-device-auth-v2\0"
|| u64_be(len(device_id)) || device_id
|| u64_be(len(nonce)) || nonce
|| u64_be(expires_at_ms)
```

签名为 Ed25519，JSON 中使用 base64 编码。nonce 默认 5 分钟有效、单次使用；无论签名验证成功与否，提交后即消费，不能重放。

#### `POST /api/v1/device/challenge`

```json
// 请求
{ "device_id": "d_42" }

// 响应
{
  "status": "success",
  "data": {
    "nonce": "ts_...",
    "expires_at_ms": 1750000000000
  }
}
```

设备不存在或所属租户已吊销时返回 404。

#### `POST /api/v1/device/session`

```json
// 请求
{
  "device_id": "d_42",
  "nonce": "ts_...",
  "signature": "<base64 Ed25519 signature>"
}

// 响应
{
  "status": "success",
  "data": {
    "session_token": "ts_...",
    "tenant_id": "t_1",
    "device_id": "d_42",
    "expires_in": 86400
  }
}
```

错误签名、过期 nonce、重复 nonce 或失效设备返回 401。Device 被删除或 endpoint_id 轮换后，旧 Device Session 失效。

## 3. 设备管理

### 3.1 `POST /api/v1/devices`

TenantAdmin Session 鉴权，注册设备到该租户。

```json
{
  "name": "家里台式机",
  "platform": "windows",
  "endpoint_id": "<iroh 公钥 hex>",
  "library_name": "我的素材库",
  "allow_remote": true,
  "allow_import": false
}
```

响应：`{ "status": "success", "data": { "device_id": "d_42" } }`。

endpoint_id 必须是有效 iroh 公钥；无效值返回 400。

### 3.2 `POST /api/v1/devices/heartbeat`

Device Session 鉴权，设备身份从 Session 取得。请求体不包含 device_id。

```json
{
  "allow_import": false,
  "library_name": "我的素材库",
  "addrs": ["192.168.1.5:27373", "1.2.3.4:27373"]
}
```

- 建议心跳间隔 30 秒，90 秒未心跳判离线，最小间隔 10 秒。
- `addrs` 是最近 iroh 直连候选，空数组表示无候选。
- 响应包含 `server_time_ms`、`public_ip_seen` 和可选 `relay`。

### 3.3 `GET /api/v1/devices`

Device Session 鉴权，返回当前设备所属租户的设备列表。列表不包含任何 ViewerHost 或全局查看设备。

### 3.4 `PATCH /api/v1/devices/{device_id}`

Device Session 鉴权，只允许更新当前认证设备；操作其他设备返回 404。

```json
{
  "name": "新名字",
  "allow_remote": false,
  "library_name": "工作库",
  "endpoint_id": "<可选：轮换后的新公钥>"
}
```

携带 endpoint_id 即轮换身份，旧 Device Session 和旧 iroh 身份会话失效。

### 3.5 `DELETE /api/v1/devices/{device_id}`

TenantAdmin Session 鉴权，删除本租户设备并取消其进行中的会话。Device 的签名 Session 立即失效。

## 4. 控制通道

### `GET /api/v1/ws`

Device Session 鉴权。URL 不再携带 device_id：

```text
wss://<base>/api/v1/ws
Authorization: Bearer <device session>
```

服务端 → Device：

| type | 内容 |
| --- | --- |
| `hello` | `{ device_id, server_time_ms }` |
| `ping` | 保活 |
| `session.request` | 新会话请求及对端 endpoint_id、role |
| `session.update` | 会话状态与 ready 连接信息 |
| `session.cancelled` | 会话取消 |

Device → 服务端只发送 `hello_ack` 和 `pong`；业务操作走 REST。新 WSS 连接顶替同设备旧连接。

## 5. Device 间会话

### 5.1 `POST /api/v1/sessions`

Device Session 鉴权。source 由 Session 决定，请求只提交目标：

```json
{
  "target_device_id": "d_42",
  "role": "viewer"
}
```

- role 为 viewer（默认）或 importer。
- 双方必须同租户，目标必须在线且 allow_remote=true。
- importer 还需要目标 allow_import=true。
- 不能与自身建立会话。

响应包含 `session_id` 和初始 state。目标经 WSS 收到 `session.request`，其中包含 source 的 device_id、展示信息和 endpoint_id；不包含任何 AccessKey 或 credential。

### 5.2 `POST /api/v1/sessions/{session_id}/accept`

仅会话 target Device 可调用，无请求体。接受后服务端组装：

```json
{
  "session_id": "s_9",
  "state": "ready",
  "connect": {
    "endpoint_id": "<target endpoint>",
    "direct_addresses": ["192.168.1.5:27373"],
    "relay": { "url": "https://<base>:9102", "token": "..." }
  }
}
```

无 relay 且无直连候选时状态为 unreachable。endpoint_id 用于 iroh 对端认证，relay 只转发端到端加密流量。

### 5.3 `POST /api/v1/sessions/{session_id}/reject`

仅会话 target Device 可调用，无请求体，状态变 rejected。

### 5.4 `GET /api/v1/sessions/{session_id}`

仅 source 或 target Device 可读取。其他 Device 即使属于同租户也返回 404。

状态：pending / ready / rejected / unreachable / expired / cancelled。

### 5.5 `POST /api/v1/sessions/{session_id}/cancel`

仅 source 或 target Device 可取消，无请求体。双方收到 `session.cancelled`。

## 6. AccessKey-only rendezvous

公网网页版已取消，AccessKey 不经过 hawk-server。原生客户端使用 server 或目标 daemon 的受限 rendezvous 建立 iroh 会话，再只在加密隧道内认证 AccessKey。

### `POST /api/v1/rendezvous`

无需 Device Session；按来源 IP、目标和 endpoint 限流。server 只创建匿名编排会话，不接收 AccessKey 或验证子。

```json
{
  "target_device_id": "d_42",
  "source_endpoint_id": "<source iroh 公钥>",
  "source_name": "手机"
}
```

响应包含 `session_id`、初始 state 和 rendezvous_token。目标 daemon 的 LAN rendezvous 使用相同路径和 DTO，但直接返回 `state=ready`，不包含任何 AccessKey。LAN 明文 HTTP 只承载有限的编排元数据，素材与 AccessKey 均进入 iroh 加密通道。

### `GET /api/v1/rendezvous/{session_id}`

使用 `Authorization: Bearer <rendezvous_token>` 查询 ready 和 connect。错误票据返回 404。`connect.endpoint_id` 必须等于客户端 profile 中钉扎的 target_identity；不一致时客户端必须取消会话并拒绝连接。

### `DELETE /api/v1/rendezvous/{session_id}`

使用同一票据取消会话并关闭目标隧道。LAN 由目标 daemon 直接清理 AccessKey SessionRegistry。

### 隧道内登录 `POST /api/v1/remote/access-session`

未认证 AccessKey 隧道只能调用此端点：

```json
{ "access_key": "hk_..." }
```

目标 daemon 按当前 library_id 验证后返回 `session_id` 和当前 role。成功前不允许读取 app/info 或素材端点。成功后 Session 引用 AccessKey key_id、当前 role 和 grant_version；删除或降权在下一请求生效。

### 隧道续期 `POST /api/v1/remote/lease`

已认证 Device/AccessKey 会话续期。目标 daemon 将租约延长 15 分钟，不改变原始 role。Device 查看方同时检查 server 会话有效；AccessKey-only 由目标 daemon 直接续期。续期失败或到期后请求返回 401。

## 7. 风控

- 认证失败按来源 IP 计数：10 分钟内 30 次失败，冷却 15 分钟，冷却中返回 429。
- nonce 过期和单次消费状态随认证调用惰性清理。
- Session 过期后不能继续使用；daemon 应重新执行设备签名，而不是重新注册设备。
- 匿名 rendezvous 按真实客户端 IP、target_device_id 和 source endpoint 限流，未认证连接只能执行登录握手。
- 反代部署必须把真实客户端地址传给 hawk-server；不得让不受信客户端伪造 Forwarded 系列头。

## 8. 已删除接口

以下接口不再存在：

- `/api/v1/viewer/*`
- `/api/v1/devices/{device_id}/keys`
- ViewerHost 设备注册与 viewer token 持久化
- 通过 hawk-server 代理 `/app`、`/assets/*` 或素材 API
- 心跳和 WSS 查询参数中的 device_id
- 会话创建中的 source_device_id、accept/reject 请求体中的 device_id
- session.request 中的 AccessKey credential

## 9. 原生传输库

`hawk-transport/` 为 Android/iOS 提供相同的 rendezvous、target_identity、iroh、AccessKey 登录、吊销和租约续期实现。移动端 UI 只访问库返回的 Loopback `api_base + credential`，不直接连接公网 viewer。

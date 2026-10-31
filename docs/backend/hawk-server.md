# hawk-server 使用指南

> hawk-server 当前提供多租户控制面、Device 会话编排和可选密文中继。公网网页版、ViewerHost、viewer token 和 server 侧 AccessKey 映射已删除。
> 字段级契约见 [remote-protocol.md](remote-protocol.md)，目标全加密架构见 [access-control-v2.md](access-control-v2.md)。

## 1. 职责与边界

| 能力 | 说明 |
| --- | --- |
| TenantAdmin | tenant_key 换有期 Session，用于设备注册和删除 |
| Device 控制面 | 设备公钥注册、Ed25519 签名挑战、Device Session、心跳、WSS 和设备自管理 |
| Device 间会话 | 同租户设备会话编排、对端 endpoint_id 和连接候选下发 |
| 内嵌中继 | 可选 iroh-relay，只转发端到端加密数据 |
| 多租户 | 设备、Device Session 和会话按 tenant 隔离，跨租户操作返回 404 |

hawk-server 不承担：

- 素材存储、索引、内容解析或最终素材授权
- AccessKey 保存、验证或同步
- 公网网页版、viewer/login、viewer token 和素材 HTTP 代理
- 用户账号、会员或多人组织体系

## 2. 构建与启动

```bash
cd hawk-server
cargo build --release --locked
cargo test

# 启动；未指定 data-dir 时默认 ~/.config/hawk-server
cargo run --

# 创建租户并打印 tenant_key（只显示一次）
cargo run -- tenant add --label home
```

部署脚本：

```bash
tools/deploy-hawk-server.sh --path /usr/local/bin/hawk-server
```

## 3. 配置

配置文件为数据目录下的 `server.toml`。优先级：CLI > 环境变量 > 配置文件 > 默认值。

| 参数 | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `--port` | `HAWK_SERVER_PORT` | 9100 | 主控制面端口，生产环境由反代终结 TLS |
| `--admin-port` | `HAWK_SERVER_ADMIN_PORT` | 9110 | 租户管理端口，仅绑 127.0.0.1，不得公开 |
| `--relay-port` | `HAWK_SERVER_RELAY_PORT` | 9102 | 内嵌 relay 的环回上游端口 |
| `--relay-url` | `HAWK_SERVER_RELAY_URL` | 空 | relay 公网入口；未配置则不提供 relay |
| `--data-dir` | `HAWK_SERVER_DATA_DIR` | `~/.config/hawk-server` | 注册表、pepper、relay token 等状态目录 |

日志写 stdout，使用 `RUST_LOG=debug` 排查。

## 4. 租户管理

tenant_key 当前沿用 `hs_` 前缀和 256-bit 随机。服务端只存 `HMAC-SHA256(pepper, tenant_key)`。

```bash
hawk-server tenant add --label home
hawk-server tenant list
hawk-server tenant revoke t_xxxx
```

也可以在仅环回的管理面执行：

```bash
curl -X POST http://127.0.0.1:9110/api/v1/admin/tenants \
  -H 'Content-Type: application/json' -d '{"label":"home"}'
curl http://127.0.0.1:9110/api/v1/admin/tenants
curl -X DELETE http://127.0.0.1:9110/api/v1/admin/tenants/t_xxxx
```

吊销租户后，TenantAdmin/Device Session 的下一请求即失效。设备记录是否保留不影响鉴权结果。

## 5. 认证与设备接入

### 5.1 tenant_key → TenantAdmin Session

```bash
BASE=https://hawk.example.com
TK=<tenant_key>

curl -s -X POST "$BASE/api/v1/tenant/session" \
  -H 'Content-Type: application/json' -d "{\"tenant_key\":\"$TK\"}"
```

TenantAdmin Session 默认 2 小时有效，用于 `POST /devices` 和 `DELETE /devices/{id}`。

### 5.2 设备公钥接入

daemon 自动生成 device_identity。使用 TenantAdmin Session 注册：

```bash
curl -s -X POST "$BASE/api/v1/devices" \
  -H "Authorization: Bearer $TENANT_SESSION" \
  -H 'Content-Type: application/json' -d '{
    "name":"家里台式机",
    "platform":"windows",
    "endpoint_id":"<iroh 公钥 hex>",
    "library_name":"我的素材库",
    "allow_remote":true,
    "allow_import":false
  }'
```

注册只返回 device_id，不返回 device_token。

### 5.3 私钥签名 → Device Session

1. `POST /api/v1/device/challenge`，提交 device_id，取得短期 nonce 和 expires_at_ms。
2. 按 [remote-protocol.md](remote-protocol.md#23-device-身份与-device-session) 的消息格式，使用设备私钥签名。
3. `POST /api/v1/device/session`，提交 device_id、nonce、base64 签名，取得 Device Session。

Device Session 默认 24 小时有效。nonce 单次使用且默认 5 分钟过期；错误签名和 nonce 重放均返回 401。daemon 会自动完成该流程并在 Session 失效后重新认证。

## 6. 控制面接口

| 方法 | 路径 | 认证主体 | 说明 |
| --- | --- | --- | --- |
| POST | `/api/v1/tenant/session` | tenant_key | 换 TenantAdmin Session |
| POST | `/api/v1/device/challenge` | 公开但限流 | 获取设备签名 nonce |
| POST | `/api/v1/device/session` | 设备签名 | 换 Device Session |
| POST | `/api/v1/devices` | TenantAdmin | 注册设备 |
| GET | `/api/v1/devices` | Device | 本租户设备列表 |
| PATCH | `/api/v1/devices/{device_id}` | Device | 仅更新当前认证设备；轮换 endpoint_id 会吊销旧 Session |
| DELETE | `/api/v1/devices/{device_id}` | TenantAdmin | 删除本租户设备 |
| POST | `/api/v1/devices/heartbeat` | Device | 心跳与候选地址；身份取 Session，无 device_id 字段 |
| GET | `/api/v1/ws` | Device | WSS 控制通道，无 device_id 查询参数 |
| POST | `/api/v1/sessions` | Device | 创建会话，只提交 target_device_id |
| GET | `/api/v1/sessions/{session_id}` | 会话参与者 | 查询状态 |
| POST | `/api/v1/sessions/{session_id}/accept` | target Device | 接受，无请求体 |
| POST | `/api/v1/sessions/{session_id}/reject` | target Device | 拒绝，无请求体 |
| POST | `/api/v1/sessions/{session_id}/cancel` | 会话参与者 | 取消，无请求体 |

已删除：`/api/v1/viewer/*`、`/api/v1/devices/{device_id}/keys`、server 素材代理和前端托管。

## 7. 数据目录

| 文件 | 内容 | 丢失影响 |
| --- | --- | --- |
| `devices.json` | 设备注册表与 tenant 归属 | Device 心跳/WS 404 后自动重新注册 |
| `tenants.json` | tenant_key 的 HMAC 和标签 | 需要重建租户并更新 tenant_key |
| `pepper` | HMAC pepper | 已存 tenant_key 哈希不可验证，必须与注册表一起备份 |
| `relay_token` | 可选 relay 准入 token | 重启后重新生成，旧 relay 会话失效 |
| `server.toml` | 服务配置 | 回退默认配置 |

Session、nonce、在线状态和进行中的会话在内存中，重启后由 daemon 自动重新认证和心跳恢复。

旧版本遗留的 `identity.json`、`viewer_tokens.json`、`keys.json` 不再被当前 server 读取，可在确认备份后删除。

## 8. 运维与排障

### 健康检查

先完成设备签名认证，再调用设备列表：

```bash
curl -s -o /dev/null -w '%{http_code}\n' \
  -H "Authorization: Bearer $DEVICE_SESSION" \
  https://hawk.example.com/api/v1/devices
```

返回 200 表示 HTTP 和 Device Session 均有效；401 应让 daemon 重新签名，403/404 按资源边界检查，429 表示触发风控。

### 设备吊销与轮换

- 临时禁止新会话：Device 调用自身 PATCH，设置 `allow_remote=false`。
- 删除设备：TenantAdmin 调用 DELETE，旧 Device Session 立即失效。
- 轮换 endpoint_id：Device 调用自身 PATCH；daemon 随后用新私钥重新认证。

### 升级

使用部署脚本覆盖二进制并重启。升级前备份整个数据目录。V2 不保留旧认证兼容层，server 与 daemon 必须同步升级到包含 Device Session 的版本。

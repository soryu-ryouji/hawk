# 权限与凭证设计

> 目标模型：轻量 Capability + 固定角色预设 + 通道权限上限。统一授权判定，不统一凭证的生命周期和信任边界。本文是实现的单一依据；旧 `writable`、`RemoteWriter` 和分散的路径白名单逐步移除，不保留兼容层。

## 目标与非目标

目标：

- 用同一套语义覆盖本机、LAN Web、LAN Peer、WAN 隧道与网页版。
- 消除 `token`、`write/writable` 在不同位置代表不同含义的问题。
- 权限由端点显式声明，新增端点默认不能因漏配白名单而意外开放。
- 配置、DTO、IPC 和代码使用同一组角色与凭证名称。

非目标：

- 用户账号、角色继承、自定义 ACL、通用权限引擎。
- 将 `service_key` 变成素材库访问凭证。
- 将文件夹/分类/标签锁并入角色体系。
- 修改 HTTP `Authorization: Bearer <token>` 和 `?token=` 这两个通用传输术语。

## 核心模型

```text
凭证认证 → Principal
访问入口 → Channel
角色预设 → 一组 Capability
设备开关 → 当前允许的能力

有效能力 = 角色能力 ∩ 通道上限 ∩ 设备/库开关
```

```rust
struct AccessContext {
    principal: Principal,
    channel: Channel,
    capabilities: CapabilitySet,
}

enum Principal {
    ProcessAdmin,
    AccessKey { id: String },
    Session { id: String },
}

enum Channel {
    Loopback,
    LanWeb,
    LanPeer,
    Wan,
}
```

`Principal` 表示凭证来源，`Channel` 表示请求穿过哪条信任边界，`capabilities` 是请求级计算后的最终能力。长期密钥吊销、角色收窄和设备开关变化必须在下一次请求立即生效。

## Capability 与角色

| Capability | 语义 | 典型端点 |
| -- | -- | -- |
| `Read` | 浏览与查询素材 | list/skeleton/detail/count/aggregate、file、thumbnail、events |
| `Import` | 新增素材及其目标目录 | item/add、item/upload、folder/create |
| `Edit` | 修改现有素材与组织方式 | update/batch_update/replace/delete/restore/trash、分类标签和文件夹修改、视图与隐藏设置 |
| `Manage` | 修改服务和访问控制 | 库扫描设置、重建索引、LAN/remote 配置、访问密钥、锁管理 |

HTTP 方法不决定能力：`POST item/list` 等复杂查询仍是 `Read`。

角色只是固定能力集合，不做继承：

| Role | Capabilities | 用途 |
| -- | -- | -- |
| `viewer` | Read | 只读浏览 |
| `importer` | Read + Import | 远程保存素材 |
| `editor` | Read + Import + Edit | LAN 素材管理 |
| `admin` | Read + Import + Edit + Manage | 本机进程管理员 |

## 通道上限

| Channel | 上限 | 说明 |
| -- | -- | -- |
| Loopback | admin | 进程 admin token 只在这里有效；本机也兼容对应 LAN key |
| LanWeb | editor | 网站密钥；禁止 Manage |
| LanPeer | editor | 已配对设备的 peer key；禁止 Manage |
| Wan | importer | iroh 隧道、会话代理和 hawk-server 网页版；禁止 Edit/Manage |

`admin` 不是可远程授予的角色。即使 access key 是 editor，经 WAN 也只剩 Read；导入还需要目标设备显式开启 `allow_import`。

## 端点授权

认证中间件只负责解析凭证并生成 `AccessContext`。业务端点显式要求能力：

```rust
access.require(Capability::Import)?;
```

- 查询端点要求 `Read`。
- 管理端点要求 `Manage`，不再单独维护 `require_admin` 的角色判断。
- 写端点未声明能力时默认拒绝（fail closed）。
- 不再使用 HTTP 路径白名单推断 viewer/remote writer 权限。
- 隧道代理只负责转发和附加受托会话信息，不另建一套素材权限语义。

## 凭证职责与命名

| 凭证 | 名称 | 生命周期与边界 |
| -- | -- | -- |
| 进程管理员凭证 | `admin_token` / `HAWK_ADMIN_TOKEN` | Electron 启动时生成；环境变量传入；不落盘；仅 Loopback |
| LAN 长期访问密钥 | `access_key` | 用户可创建、改角色和吊销；LanWeb/LanPeer |
| LAN 配对回连密钥 | `peer_key` | 每台设备独立；保存在本机回连 profile |
| WAN 长期访问密钥 | `access_key` | 用于签发远程会话；daemon 只存验证哈希 |
| 远程会话临时凭证 | `session_token` | 随会话创建和销毁；只存内存；绑定目标、通道与角色 |
| 多形态连接凭证 | `credential` | ConnectionDescriptor 的通用字段，可能是 admin/access/session 凭证 |
| 租户接入密钥 | `service_key` | 只允许 hawk-server 租户控制面，不直接授予素材 Read |
| 门令牌 | `gate_token` | 网页版登录第一阶段 |
| 浏览器令牌 | `browser_token` | 网页版会话凭证；服务端只存哈希 |
| 解锁票据 | `unlock_token` | 只扩大锁内容可见范围，不授予角色能力 |

### admin token 发现

`GET /api/v1/app/admin-token` 替代 `/app/token`，响应字段为 `admin_token`。仍仅用于本机扩展零配置发现：Host 限定环回、响应不带 CORS、WAN 隧道拒绝转发。

### 连接描述符

```text
ConnectionDescriptor {
  api_base,
  credential,
  label,
  kind: local | remote | lan,
  ...
}
```

前端不需要知道 credential 的具体类型；主进程和 daemon 在认证边界知道。

## 配置目标

### LAN access key

长期密钥用 `role` 替代 `write`：

```toml
[[web.keys]]
id = "key_..."
label = "手机"
secret = "..."
role = "viewer" # 或 editor

[[web.peer_keys]]
id = "key_..."
label = "办公室电脑"
secret = "..."
role = "editor"
```

当前密钥仍位于库配置并随库同步；将密钥迁出库同步边界、验证侧只存哈希是后续存储加固，不与本次授权模型耦合。

### WAN access key

`keys.json` 使用 `role: viewer | importer`，替代 `read/write` 两个布尔值。现有明文只显示一次、daemon 只存 SHA-256 验证子的行为保持不变。

### 远程设备开关

```toml
# remote.toml
allow_import = false
```

替代 `writable`。它只允许 WAN `Import`，不影响 Read，也不会开放 Edit/Manage。

## 会话与吊销

- 会话在签发时确定 `viewer` 或 `importer`，通道固定为 Wan。
- `session_token` 只用于本机会话代理，不是长期 access key。
- access key 吊销或角色收窄后，新会话立即按新映射执行；已有会话必须在目标 daemon 的会话放行集合中同步撤销或降权。
- `allow_import=false` 每次请求生效，已有 importer 会话立即退化为 viewer。
- LAN 权限变化继续通过 SSE 通知客户端重新读取 `app/info`。

## 锁与权限的关系

锁是行级可见性过滤，不是角色：

```text
Capability::Read 通过 → LockGuard 过滤可见条目
```

解锁票据只扩大本次请求可见范围，不授予 Import/Edit/Manage，也不绕过通道上限。

## 可观测契约

`app/info` 使用：

```json
{
  "role": "editor",
  "channel": "lan_peer",
  "has_library": true
}
```

替代旧的 `access`、`writable`、`remote` 三字段。前端按 role 展示写入口，按 channel 判断是否采用低带宽策略。

远程会话 DTO 使用 `session_token` 和 `role`；LAN/远程访问密钥 DTO 使用 `role`，不再返回 read/write 布尔组合。

## 实施边界与提交批次

1. 设计文档。
2. daemon 基础 AccessContext、端点能力判定、admin/credential/session 命名及 app 对齐。
3. LAN access key/peer key 角色化。
4. WAN access key、allow_import、hawk-server 与 remote protocol 对齐。
5. OpenAPI/TypeScript 产物、客户端和文档收尾。

每批不保留旧字段兼容层，并运行对应 Rust、Web、契约和文档检查。存储位置迁移、OS keychain 和账号体系列为后续独立工作。

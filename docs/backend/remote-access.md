# 远程访问设计

> 当前处于权限与传输 V2 改造中。目标架构的完整依据是 [access-control-v2.md](access-control-v2.md)，当前已实现的控制面接口见 [remote-protocol.md](remote-protocol.md)。
> 公网网页版已取消并从代码删除，不再作为部署或产品能力提供。

## 1. 目标形态

```text
设备 A（素材库所在机）                         设备 B（查看方）
┌──────────────────────┐                     ┌──────────────────────┐
│ hawk-app             │                     │ hawk-app / 原生客户端 │
│  └─ HTTP Loopback ─┐ │                     │  └─ HTTP Loopback ─┐ │
└────────────────────┼─┘                     └────────────────────┼─┘
                     ▼                                          ▼
              hawk-daemon                                hawk-daemon
              API + remote                               remote 会话代理
                     ▲                                          │
                     └──── iroh 端到端加密隧道（直连或密文中继）──┘
                                      │
                               hawk-server
                    TenantAdmin/Device 控制面、寻址、rendezvous
```

目标边界：

- 素材数据始终由目标 daemon 产出。
- 桌面与原生客户端 LAN/WAN 均通过 iroh 端到端加密访问目标 daemon。
- relay 和 hawk-server 不终结素材隧道，只能看到密文。
- 取消公网网页版；server 不保存 AccessKey、不签发 viewer token、不代理素材明文。
- 同一客户端使用一份 `AccessKey + target_identity + LAN/WAN endpoints` profile。
- 目标 daemon 始终执行最终素材授权。

## 2. 当前实施状态

### 已完成

- TenantAdmin Session：`tenant_key → tenant/session`。
- 设备公钥接入和 Ed25519 签名挑战：`device/challenge → device/session`。
- Device Session 用于注册后的心跳、WSS、设备自管理和 Device 间会话。
- 心跳、WSS、accept/reject 不再从请求体或 URL 信任 device_id。
- 会话查询和取消仅允许 source/target Device，不能仅凭同租户身份操作。
- 公网 ViewerHost、viewer token、viewer/login、server AccessKey 映射及同步端点已删除。
- daemon 使用 device_identity 自动完成签名认证和 Session 续期。
- OpenAPI 与前端类型已同步，相关 Rust/Web 测试通过。

### 已完成后续阶段

- 统一 daemon Principal/Grant/Session/AccessContext 和 AccessKey 逐请求降权。
- target_identity 钉扎、AccessKey-only rendezvous 和隧道租约续期。
- 原生 LAN Peer 已迁移 iroh，LAN/WAN 共用加密隧道；旧明文 Peer 只保留配对/rendezvous 控制面。
- 共享 Rust 移动传输库 `hawk-transport`，Android/iOS 通过 Loopback 代理复用同一实现。
- LAN Web 已切换 HTTPS 和 Session Cookie；URL token 与媒体专用票据已移除。
- 本机 LocalIntegration 与统一 AccessKey 存储已完成，网站 key 不再从库内配置作为认证依据。

## 3. 凭证职责

| 凭证/身份 | 用途 | 是否接触素材数据 |
| --- | --- | --- |
| tenant_key | 所有者换取 TenantAdmin Session，接入/删除设备 | 否 |
| device_identity | 设备签名认证和 iroh 对端身份 | 由 daemon 自动使用 |
| Device Session | 心跳、WSS、设备自管理、Device 间会话 | 仅控制面 |
| AccessKey | 客户端在加密通道内向目标 daemon 证明素材访问权限 | 是，且只出现在客户端与目标 daemon 之间 |
| session_token | 有 audience 的短期认证状态 | 按 audience 限定 |
| admin_bootstrap | Electron 与本机 daemon 的管理握手 | 仅 Loopback |

不再存在 device_token、viewer browser token、gate token 或 server 侧 AccessKey 验证子。

## 4. 组件职责

### hawk-daemon

- 本地素材 API、角色/通道/资源授权和 AccessKey 最终执法。
- 维护 device_identity，自动完成设备签名认证。
- remote 模块负责 Device 控制面、iroh 数据面和本机远程会话代理。
- 后续在加密隧道内直接完成 AccessKey 登录，不经过 server。

### hawk-server

- 租户密钥注册表和 TenantAdmin Session。
- 设备公钥注册、签名挑战、Device Session。
- 心跳、在线状态、WSS 和 Device 间会话编排。
- 受限 rendezvous 与可选 iroh relay；relay 只处理密文。
- 不包含 viewer/、公网前端、AccessKey 注册表或素材代理。

### hawk-app 与原生客户端

- 保存统一连接 profile 和必要的长期秘密。
- 网络切换时自动重建加密 Session，不要求用户再次输入 AccessKey。
- 桌面端由本机 daemon 承担 iroh；移动端使用共享 Rust 传输库。
- 浏览器插件远程保存通过本机 daemon，不再直接访问公网 viewer API。

## 5. 数据面零信任边界

本设计中的“零信任 server”特指**素材数据面不可读**：

- 文件、缩略图、标签、备注、搜索、导入和 SSE 素材事件均端到端加密。
- server 数据库、日志和 relay 流量不应出现 AccessKey 或素材明文。
- server 仍可见 tenant/device ID、IP、在线状态、连接时间、流量大小和会话关系。
- Device 成员注册仍信任 hawk-server；server 可以拒绝服务。
- AccessKey 客户端通过带外 target_identity 防止 server 用错误 endpoint 冒充目标。

## 6. LAN 浏览器

公网网页版与 LAN 浏览器是两个独立能力。取消公网网页版不意味着必须取消 LAN 查看，但 LAN 浏览器也必须改为：

- HTTPS + 目标证书身份
- AccessKey 换发的 Session Cookie（HttpOnly、SameSite=Strict、Secure）
- 不再支持明文 HTTP 或 URL `?token=`

该部分尚未实施，按 `plan.md` 后续阶段交付。

## 7. 相关文档

- [权限与传输架构 V2](access-control-v2.md)：目标模型和验收不变量
- [远程访问协议 V2](remote-protocol.md)：当前控制面接口
- [hawk-server 使用指南](hawk-server.md)：运行和租户管理
- [远程访问部署](remote-deployment.md)：TLS 反代与 relay
- 根目录 `plan.md`：当前实施进度

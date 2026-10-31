# 权限与传输架构 V2（实施目标）

> 状态：目标模型已落地。本文同时记录安全边界与验收依据；各阶段测试和全量质量门禁通过后作为 V2 单一契约。
> 已确认并实现：取消公网网页版；hawk-server 不再终结素材隧道；原生客户端 LAN/WAN 均使用 iroh 端到端加密，AccessKey 只在客户端与目标 daemon 之间验证。
> 协议、DTO、客户端和文档按目标模型直接重写，不保留旧认证路径或兼容层。

## 当前实现摘要

- TenantAdmin/Device Session、设备 Ed25519 签名挑战、nonce 单次消费和设备/会话资源隔离已上线。
- AccessKey 统一 `library_id + key_id + grant_version`，LAN/WAN 共用，逐请求吊销和降权。
- AccessKey-only rendezvous、target_identity 钉扎、隧道内登录和 15 分钟租约续期已打通。
- 原生 LAN/WAN 使用 iroh 端到端加密；LAN Web 使用 HTTPS 与 Session Cookie，旧明文 Peer 不再提供素材 API。
- `hawk-transport` 为 Android/iOS 提供相同 Loopback 代理实现；浏览器插件统一经本机 daemon。
- AccessKey 存储迁出库同步边界，LocalIntegration 不再返回进程 admin token，URL token 已删除。

## 1. Problem Statement

### 1.1 目标

为 hawk 的本机、LAN、WAN 设备互联和插件建立一套简单、统一、加密的权限与传输架构：

- 用户只需理解两种 key：所有者的 `tenant_key` 和素材访问的 `access_key`。
- 同一客户端访问同一素材库时，LAN/WAN 共用 AccessKey 和 target_identity。
- 原生客户端 LAN/WAN 都通过 iroh 建立端到端加密通道；relay 只能看到密文。
- 取消公网网页版。hawk-server 不再终结素材隧道，不读取或代理素材明文。
- AccessKey 只在客户端与目标 daemon 的加密通道内验证，hawk-server 不保存 AccessKey 登录映射。
- 设备使用 Ed25519/iroh 私钥签名认证，删除独立 device_token。
- 所有短期认证状态统一为带 audience 的 session_token。
- 目标 daemon 始终是素材权限、AccessKey 和本机开关的最终执法者。
- 吊销、降权和设备开关变化具有确定、可测试的生效语义。
- 跨租户、跨设备、跨会话和跨 audience 访问默认拒绝。

### 1.2 当前问题

现行实现与目标的主要差异：

- tenant-key 同时承担租户管理、设备控制和查看者登录，凭证职责混用。
- 控制面只认证租户，没有认证具体设备，设备身份由请求体自报。
- Session 被当作独立 Principal，隧道只缓存 role，已有会话不能完整响应吊销和降权。
- hawk-server 的 ViewerHost 会终结素材隧道，公网网页版数据对 server 是明文。
- 公网网页版要求 server 保存 AccessKey 映射并签发 browser token，扩大了凭证面。
- LAN Web/Peer 使用明文 HTTP，不能防窃听或主动冒充目标。
- tenant/device/session 资源授权分散在 handler 中。

### 1.3 安全边界

本设计承诺：

- 素材文件、缩略图、路径、标签、备注、搜索内容、导入内容和 SSE 素材事件在 LAN/WAN 传输中均加密。
- iroh relay 和 hawk-server 数据面代理只能看到密文。
- AccessKey 只在目标 daemon 与客户端之间出示和验证，不发送给 hawk-server。
- AccessKey 客户端通过 profile 中的 target_identity 验证目标公钥，server 不能通过替换寻址结果静默冒充该目标。

本设计不承诺：

- hawk-server 无法看到 tenant/device ID、IP、在线状态、连接时间、流量大小和对端关系。
- hawk-server 无法拒绝服务、丢弃消息或使设备离线。
- 同 tenant Device 的成员关系完全不信任控制面；Device 控制面仍以 hawk-server 为受信任注册机构。
- 本机 Loopback 也必须加密；Loopback 以操作系统进程边界和最小权限凭证为边界。

如未来要求 Device 成员关系也不信任 server，可在现有 Grant 上增加所有者签名的设备成员证明，不改变素材授权模型。

### 1.4 产品边界调整

- 删除 hawk-server 公网网页版及其 ViewerHost、viewer token、素材代理和前端托管。
- 浏览器插件不再直接经公网 viewer API 保存素材；远程保存通过本机 daemon 发起加密会话。
- LAN 浏览器查看可以保留，但必须使用 HTTPS 和目标证书身份；不再提供 LAN 明文 HTTP 查看。
- 桌面端由本机 daemon 承担 iroh 客户端；iOS/Android 通过共享 Rust 传输库使用同一协议，不另发明移动端鉴权。

## 2. 最小凭证模型

### 2.1 用户只需理解两种 key

| 凭证 | 持有者 | 用途 | 明确不能做什么 |
| --- | --- | --- | --- |
| `tenant_key` | 素材库所有者 | 接入和管理自己的设备 | 不授予素材读取权限，不分发给查看者 |
| `access_key` | 被授权的客户端 | 访问绑定的素材库；LAN/WAN 共用 | 不能管理 tenant、设备或 daemon |

两种 key 都只在首次配置或分享时出现，之后由客户端安全保存并自动使用。

### 2.2 系统自动维护两种长期身份材料

| 身份材料 | 认证为 | 来源 | 用户是否感知 |
| --- | --- | --- | --- |
| `device_identity` | Device | daemon 自动生成的 Ed25519/iroh 密钥对 | 否 |
| `admin_bootstrap` | ProcessAdmin | Electron 启动 daemon 时生成并注入 | 否 |

- device_identity 同时用于控制面设备认证和 iroh 数据面身份，不再额外发放 device_token。
- admin_bootstrap 只用于 Electron 主进程换取本机管理 Session，不能经发现端点或渲染进程暴露。

### 2.3 只有一种会话凭证

```text
session_token
```

本机 UI、TenantAdmin、Device、LAN/WAN AccessKey 和远程代理统一使用不透明 session_token。以下名称不再表示独立凭证类型：

- browser_token / proxy_token / local_session_token
- integration_token
- media_ticket
- tunnel_token

它们如需保留，只能是 Session 的 audience、用途或存储位置说明。

### 2.4 不属于认证凭证的概念

- session_id：会话标识符，不是秘密；隧道还必须校验 iroh 对端身份。
- target_identity：目标公钥/指纹，用于认证目标，不是秘密。
- grant_version：授权版本，不是凭证。
- 配对 request_id、设备挑战 nonce：一次性流程参数，不是长期凭证。
- 解锁票据：只扩大锁内容可见范围，不属于登录或角色权限体系。
- relay 内部准入配置：传输组件配置，不是用户或素材库身份。

## 3. 核心授权模型

```text
Credential  凭证：证明调用方持有秘密或私钥
Principal   主体：稳定、可审计的身份
Grant       授权：主体对资源的角色与范围
Session     会话：有时限的认证状态，引用 Principal 和 Grant
Channel     通道：请求穿过哪条网络信任边界
Capability  能力：允许执行哪类素材操作
Resource    资源：tenant、device、library、session 等授权对象
```

请求处理链固定为：

```text
加密传输与对端认证 → Session 认证 → Grant 解析 → 资源与能力授权 → 业务 Handler
```

最终授权公式：

```text
1. 对端身份必须符合当前 Channel 和 Session
2. Principal 必须存在且认证未失效
3. Session 必须有效，并仍引用有效的 Principal 与 Grant
4. Session.audience 必须覆盖当前 API 面
5. ResourceScope 必须包含请求目标资源
6. effective_capabilities =
       role_capabilities
       ∩ channel_ceiling
       ∩ resource_policy
7. 请求 Action 必须属于 effective_capabilities
```

任何一项无法确定都拒绝。

### 3.1 Principal

```rust
enum Principal {
    ProcessAdmin,
    TenantAdmin { tenant_id: String },
    Device { tenant_id: String, device_id: String },
    AccessKey { key_id: String },
    LocalIntegration { installation_id: String },
}
```

| Principal | 含义 | 典型调用方 |
| --- | --- | --- |
| ProcessAdmin | 本机 daemon 进程管理员 | Electron 主进程、本机管理 UI |
| TenantAdmin | 某租户的所有者 | 设备接入和租户设备管理 |
| Device | 已完成接入的一台设备 | daemon 控制面和设备间会话 |
| AccessKey | 对单库的一把访问授权 | 桌面、手机、插件；LAN/WAN 共用 |
| LocalIntegration | 本机受限生态客户端 | 浏览器插件本地保存和读取当前连接 |

LocalIntegration 是 Session-only 主体，不签发长期 integration_token。

AccessKey 的身份只需要稳定 key_id；tenant_id 是可选的 WAN 路由与隔离信息，属于连接元数据，不是 LAN 认证成立的前提。因此未接入 hawk-server 的库仍能离线使用同一套 AccessKey 模型。

### 3.2 Session 不是 Principal

```text
Session {
    session_id,
    principal,
    audience,
    grant_id,
    grant_version,
    resource_scope,
    channel,
    issued_at,
    expires_at
}
```

Session 是认证状态，不是新的业务身份。每个 Session 必须追溯到稳定 Principal。

| Audience | 允许的 API 面 |
| --- | --- |
| `local_admin` | 本机 daemon 管理 API |
| `tenant_control` | 租户设备管理 |
| `device_control` | 本设备心跳、WSS、会话编排 |
| `library` | LAN HTTPS 素材 API |
| `remote_proxy` | 本机 Loopback 远程会话代理 |

一种 session_token 不代表一个 token 可以通行所有 API；隔离由 audience、ResourceScope 和 Channel 共同完成。

## 4. 认证与连接流程

### 4.1 本机管理

```text
Electron 生成 admin_bootstrap 并注入 daemon
  → 主进程经 Loopback 换取 audience=local_admin 的 Session
  → 渲染进程只获得该 Session 或经主进程代理请求
```

- admin_bootstrap 不进入渲染进程、localStorage、二维码或插件发现接口。
- 本机插件通过 Loopback 发现获得 LocalIntegration Session，而不是 ProcessAdmin Session。
- 所有远程隧道拒绝访问本机 bootstrap 和集成发现端点。

### 4.2 设备接入

```text
daemon 生成 device_identity
  → 所有者使用 tenant_key 建立 TenantAdmin Session
  → 提交 endpoint_id（公钥）接入设备
  → server 创建 Device { tenant_id, device_id }
```

接入响应只返回 device_id，不返回 device_token。

### 4.3 设备控制面认证

设备复用已有 Ed25519 私钥，通过签名挑战换 Device Session：

```text
POST device/challenge { device_id }
  → server 返回短期、单次 nonce

daemon 签名 domain_separator || device_id || nonce || expires_at
  → POST device/session { device_id, nonce, signature }
  → server 验签并返回 audience=device_control 的 session_token
```

- nonce 短期、单次使用，签名带域分隔。
- 心跳、WSS、设备信息更新和会话编排均使用 Device Session。
- DeviceAuth 直接产生 `{ tenant_id, device_id }`，请求体不再承担身份声明。
- source_device_id 必须等于认证主体；accept/reject 只能由 target Device 执行。
- Device Session 过期后，daemon 用私钥自动重新认证，无用户操作。
- 吊销 Device 同时撤销其公钥注册、Device Session 和关联隧道。

### 4.4 AccessKey 认证

AccessKey 由目标 daemon 签发：

```text
key_id
target_library_id
target_identity
tenant_id?             # 仅 WAN 接入后存在
target_device_id?      # 仅 WAN 接入后存在
role = viewer | importer | editor
verifier
grant_version
created_at
```

- 目标 daemon 是 AccessKey 是否存在、当前 role 和 grant_version 的唯一权威来源。
- 同一 key_id 和 secret 可用于该库已启用的 LAN/WAN，不拆成 web/peer/remote key。
- AccessKey 只在已认证 target_identity 的加密通道内出示，由目标 daemon 直接验证并签发素材 Session。
- hawk-server 不同步 AccessKey、不保存验证子、不提供 viewer/login，也不决定 AccessKey role。
- LAN HTTPS 查看由目标 daemon 直接完成 AccessKey 登录；原生 LAN/WAN 客户端在 iroh 隧道内完成登录。
- AccessKey 降权或删除不需要等待 server 同步。

### 4.5 一套 profile 跨 LAN/WAN

```text
ConnectionProfile {
    target_library_id,
    target_device_id?,      // WAN 寻址需要
    target_identity,        // 目标 endpoint 公钥/指纹
    access_key,
    lan_endpoints,
    wan?: { server_url, tenant_id }
}
```

配对二维码只交换这一份 profile，不要求用户分别完成 LAN/WAN 配对。

```text
LAN 可达 → iroh 直连 target_identity → 加密通道内验证 AccessKey
LAN 不可达 → server 寻址/中继 → iroh 连接同一 target_identity → 验证同一 AccessKey
网络切换 → 结束旧 Channel Session → 自动建立新加密通道和 Session
```

协议层重新认证是必要边界，但不等于用户再次输入或审批。

target_identity 在首次配对时带外传递，客户端连接前必须校验。仅知道目标 IP、device_id 或 server 返回的 endpoint_id 不能覆盖 profile 中已钉扎的 target_identity。

### 4.6 外部 AccessKey 会话编排

 hawk-server 可以为 AccessKey 客户端提供受限 rendezvous，但不认证素材权限：

- 客户端生成自己的 iroh 身份，并提交 target_device_id、source endpoint 和连接候选。
- server 只负责限流、转发会话请求和返回目标候选，不接收 AccessKey。
- 目标 daemon 在加密通道内完成 AccessKey 认证后才授予素材 Session。
- 未认证连接只能执行严格的登录握手，不能读取库信息或调用素材端点。
- 客户端必须按 profile 校验 target_identity，避免恶意或错误寻址导致连接到其他 endpoint。
- rendezvous 按真实客户端 IP、target_device_id 和 source endpoint 限流，防止匿名会话请求消耗目标资源。

## 5. 加密传输

### 5.1 通道矩阵

| Channel | 传输 | 对端认证 | 能力上限 |
| --- | --- | --- | --- |
| Loopback | 本机 HTTP/进程内调用 | admin_bootstrap 或本地 Session | admin |
| LanWeb | HTTPS | 目标 TLS 证书身份 | editor |
| LanPeer | iroh 直连 | target_identity / Device 公钥 | editor |
| Wan | iroh 直连或 relay | target_identity / Device 公钥 | importer |

Channel 由监听器或可信隧道适配器写入，客户端不能通过 HTTP 头伪造。

### 5.2 原生客户端统一使用 iroh

- 桌面端继续由本机 daemon 负责 iroh 建连和 Loopback remote_proxy。
- iOS/Android 通过共享 Rust 传输库使用相同 ALPN、target_identity、AccessKey 登录和 Session 协议。
- 共享库只负责传输和会话，不包含素材业务授权；目标 daemon 仍统一执法。
- LAN 与 WAN 复用同一实现，仅候选地址、relay 和发现方式不同。
- relay 只转发 iroh 密文，不终止 TLS，不读取 HTTP 或其他素材载荷。

### 5.3 LAN 浏览器 HTTPS

LAN Web 是独立保留的浏览器入口：

- daemon 生成独立 TLS 身份或复用可映射到 target_identity 的证书。
- 配对信息包含证书指纹；原生客户端自动钉扎。
- 普通浏览器不能静默钉扎自签证书，首次访问需要用户显式核对并信任目标证书。
- 不提供“忽略证书错误后仍视为安全”的应用逻辑，也不再回退到 LAN 明文 HTTP。
- WAN 不提供该 HTTP 查看入口，公网网页版已取消。

### 5.4 Session 传输

普通 API 使用：

```http
Authorization: Bearer <session_token>
```

LAN HTTPS 浏览器的页面、图片和 EventSource 使用同一 Session 的 Cookie：

```http
Set-Cookie: hawk_session=<session_token>;
  HttpOnly; SameSite=Strict; Secure; Path=/
```

- Cookie 必须带 Secure，因为 LAN Web 也只接受 HTTPS。
- Cookie 名加入 issuer 或监听端口后缀，避免同主机多端口互相覆盖。
- Cookie 写请求必须校验 Origin，并要求自定义请求头触发 CORS 预检。
- 不接受 `?token=` 作为通用认证方式，不生成 media_ticket。
- 原生客户端使用 Bearer 或标准 CookieJar，不另建传输凭证类型。

## 6. 素材授权

### 6.1 Capability 与 Role

| Capability | 语义 |
| --- | --- |
| Read | 浏览、查询、事件、文件和缩略图 |
| Import | 新增素材及创建导入目标目录 |
| Edit | 修改、替换、删除、恢复和组织现有素材 |
| Manage | 修改 daemon 服务和访问控制配置 |

固定角色，不做角色继承或自定义角色：

| Role | Capabilities |
| --- | --- |
| viewer | Read |
| importer | Read + Import |
| editor | Read + Import + Edit |
| admin | Read + Import + Edit + Manage |

| 主体 | 可授予 Role |
| --- | --- |
| ProcessAdmin | admin |
| Device，WAN | viewer / importer |
| AccessKey，所有通道 | viewer / importer / editor；最终能力由 Channel 收窄 |
| LocalIntegration | importer，另受专用端点范围限制 |

admin 不是任何远程凭证可获得的角色。签发接口必须在入口拒绝不适用的 Role。

同一把 editor AccessKey 在 LanWeb/LanPeer 可获得 editor，在 Wan 最高为 importer，并继续受 allow_import 和 WAN 端点限制。

### 6.2 ResourceScope 与 ResourcePolicy

```text
ProcessAdmin       → 本机 daemon 与当前本地库
TenantAdmin        → 指定 tenant 的控制面资源
Device             → 自身设备资源 + 同 tenant 设备会话准入
AccessKey          → 指定 target_library/device
LocalIntegration   → 本机集成端点与当前连接的受限描述
```

ResourcePolicy 至少包括：

- LAN Web/Peer 是否启用
- allow_remote
- allow_import
- 目标设备/库是否存在且可用
- Session 是否仍处于有效状态

WAN allow_import=false 只移除 WAN Import，不影响 Read，也不改变 LAN AccessKey 权限；每次请求重新计算。

文件夹、分类和标签锁继续作为 Read 通过后的可见性过滤。解锁票据不能授予 Import/Edit/Manage。

### 6.3 端点与控制面资源授权

每张路由只声明一个明确 Action 和目标资源提取方式：

```text
item.list       → Read(Library)
item.upload     → Import(Library)
item.update     → Edit(Library)
remote.keys     → Manage(LocalDaemon)
device.update   → Update(DeviceSelf)
session.accept  → Accept(SessionTarget)
```

统一 `authorize(context, action, resource)` 同时完成 Session/Grant 有效性、资源归属和能力交集。未声明 Action 或资源提取规则的端点默认拒绝，并由契约测试覆盖全部端点。

| 控制面操作 | 允许主体 |
| --- | --- |
| 接入、列出、吊销租户设备 | TenantAdmin |
| 心跳、WSS、更新自身 | 对应 Device |
| 发起设备间会话 | source Device；target 必须同 tenant |
| 查看会话 | source/target Device，或对应 AccessKey 会话的本地代理 |
| accept/reject | target Device |
| cancel | source/target Device，或对应 AccessKey 会话的本地代理 |
| AccessKey rendezvous | 未认证客户端，仅限流和传输编排，不授予素材能力 |

“同租户”不能替代“本人/本设备/本会话参与者”检查。

## 7. 会话、隧道与吊销

### 7.1 AccessContext

```text
AccessContext {
    principal,
    session_id,
    audience,
    grant_id,
    grant_version,
    role,
    channel,
    resource_scope,
    effective_capabilities
}
```

删除 Principal::Session 和只含 role 的 TunneledPeer。隧道适配器只提供 session_id、对端 endpoint_id 和 Channel；授权层从 SessionRegistry 解析完整上下文。

### 7.2 隧道认证

iroh 握手认证 source_endpoint_id，session_id 选择 AcceptedSession，对端身份必须与 AcceptedSession 一致。

AcceptedSession 至少保存：

```text
session_id
source = Device { ... } | AccessKey { ... }
source_endpoint_id
target_device_id
requested_role
grant_version
lease_expires_at
state
```

AccessKey 客户端在完成隧道内登录前只能是 Unauthenticated 状态，且只能调用登录握手。B 侧 Loopback 代理使用 audience=remote_proxy 的普通 session_token。

### 7.3 请求级复核

每个 WAN 素材请求都重新计算：

- AccessKey 会话：key 必须仍存在；当前 key role 覆盖原请求角色时才保留该角色，否则收窄。
- Device 会话：Device Grant 和会话租约必须有效。
- Import：除有效角色外，还必须满足当前 allow_import=true。
- Edit/Manage：始终被 WAN Channel 拒绝。

AccessKey 删除使下一请求失败并关闭会话；角色收窄立即影响下一请求。已有会话不能只使用建立时缓存的 role。

### 7.4 有界租约

仅依赖 WSS 取消通知不足以保证断线场景下的设备吊销。隧道采用可续期租约：

- 默认租约 15 分钟，调用方在到期前透明续期。
- 续期时 server 重新检查 Device/tenant 状态和会话参与者，目标 daemon 重新检查本地 AccessKey/设备策略。
- AccessKey-only 会话不依赖 server 续期，直接由目标 daemon 按本地 Grant 续期。
- 续期失败或租约到期，目标关闭会话；无需前端重新选择设备。
- AccessKey 删除、降权和 allow_import 变化不等待租约，仍在下一请求立即生效。
- Device/tenant 吊销优先通过取消事件立即生效，租约是断线漏事件时的最坏时限兜底。

租约是授权有效期，不是连接保活时间。

### 7.5 Session 生命周期

不同 audience 可以有不同的过期策略，但仍使用同一 Session 模型：

- 本机管理 Session：随 daemon/UI 生命周期失效。
- Device 控制 Session：短期有效，私钥自动续认证。
- AccessKey 查看 Session：默认 30 天绝对过期，也可由目标提前吊销。
- 远程代理 Session：随隧道断开销毁。
- LocalIntegration Session：本机进程或客户端会话期有效。

Session 验证时必须检查主体、Grant、grant_version、audience、resource_scope 和 expires_at。logout 同时删除 Session、关闭关联隧道并取消目标会话。

### 7.6 吊销语义

| 变更 | 新登录/新会话 | 已有会话 |
| --- | --- | --- |
| AccessKey 删除 | LAN/WAN 目标立即拒绝 | LAN/WAN 下一素材请求拒绝；同时尽力主动断开 |
| AccessKey 降权 | LAN/WAN 目标立即按新 role | LAN/WAN 下一请求降为当前有效 role |
| WAN allow_import=false | 不允许 WAN importer，不影响 LAN | 下一 WAN 请求移除 Import |
| Device 吊销 | 立即拒绝签名认证和新会话 | 取消事件立即收尾；最迟不超过隧道租约 |
| tenant 禁用 | server 控制面与 WAN Device 会话拒绝；不隐式撤销 AccessKey | 取消 Device 控制会话；最迟不超过隧道租约 |
| tenant_key 轮换 | 不影响已接入 Device | 只撤销 TenantAdmin，不隐式吊销 Device |
| Session 过期/注销 | 立即拒绝 | 关联隧道同步关闭 |

“立即”指目标权威节点完成状态变更后的下一次请求，不依赖 server 同步或客户端主动刷新。

## 8. 存储边界

- tenant_key、AccessKey 明文只保存在需要再次出示它们的客户端安全存储中。
- daemon 只需验证的 bearer credential 只存验证子；验证子不能反过来作为 Session 凭证。
- hawk-server 不保存 AccessKey 或其验证子。
- device_identity 私钥存应用级非同步目录；移动端长期 AccessKey 使用 Keychain/Android Keystore 加密存储。
- admin_bootstrap 只存在于 Electron 主进程与 daemon 的启动注入通道，不进入渲染进程。
- AccessKey 从库内 `.hawk/config.toml` 迁出，不随素材库或网盘同步，也不按 web/peer/remote 分表复制授权。
- AccessKey 明文只在签发时返回一次；重新分享通过轮换完成。
- 长期秘密使用操作系统 CSPRNG 生成，默认 256-bit 随机，不再支持低长度自定义访问密钥。
- 凭证比较使用恒定时间比较；哈希、HMAC、签名和加密传输使用成熟库，不自行实现密码学原语。

## 9. 组件职责与代码分层

### 9.1 hawk-daemon

```text
api/ingress       Loopback、LanWeb HTTPS、Cookie/Bearer
api/authn         admin bootstrap、AccessKey、Session 认证
api/session       Session → Principal/Grant、audience、过期
api/policy        Action、ResourceScope、Role/Channel/Policy 授权
core/access       Principal、Grant、Role、Capability 等领域类型
remote/identity   device_identity 与 target_identity 校验
remote/data       iroh 对端认证、LAN/WAN 加密流转发
remote/session    会话编排、AccessKey 登录、续租、本地代理
remote/keys       AccessKey 签发、版本、吊销；不再同步 server
```

### 9.2 hawk-server

```text
authn             tenant_key、设备签名、Device Session 认证
policy            tenant/device/session 控制面资源授权
identity          设备公钥注册
control           设备接入、心跳、WSS、会话编排与续租
rendezvous        AccessKey 客户端的受限寻址和限流
relay             仅密文中继
```

明确删除 viewer/、ViewerHost、viewer_tokens、AccessKey 注册表、viewer/login、素材代理和公网前端托管。

### 9.3 共享传输与客户端

- 共享 Rust 传输库封装 iroh、target_identity、AccessKey 隧道登录和 Session 协议，供移动端绑定使用。
- Electron 主进程持有 admin_bootstrap 和其他长期秘密；渲染进程不持久化长期凭证。
- 移动端每个目标库只保存一份含 AccessKey、target_identity 与 LAN/WAN 连接信息的 profile。
- 通道切换复用 profile 自动重新认证，并重新读取 role/channel/capabilities。
- 本机插件只获得 LocalIntegration Session；远程保存经本机 daemon，不直接访问公网 viewer。
- 前端只消费当前权限快照，不自行推断权限交集；目标 daemon 拒绝仍是最终边界。

连接描述符保持统一：

```text
ConnectionDescriptor {
    api_base,
    session_token,
    label,
    kind,
    role,
    channel
}
```

## 10. 风控与审计

- 统一 ClientIp 提取器：仅当 TCP 对端属于配置的受信反代时，才接受 Forwarded 系列头。
- 设备挑战、控制面和匿名 rendezvous 按真实客户端 IP、tenant、device_id、source endpoint 限流。
- nonce 短期、单次使用；限速和 nonce 状态定期清理，避免无界内存增长。
- 认证失败统一返回通用无效凭证，避免泄露 tenant/device/key 是否存在。

审计至少记录：

```text
时间、principal.type/id、tenant_id、session_id、audience、channel、
action、resource、grant_version、允许/拒绝、拒绝原因
```

不得记录 Authorization、Cookie、AccessKey、Session 明文、验证子或带凭证的 URL。relay 日志不得包含素材载荷，因为 relay 只处理密文。

## 11. 明确删除的设计

- 公网网页版和 hawk-server ViewerHost
- hawk-server viewer token、AccessKey 映射、viewer/login 和素材代理
- tenant-key 作为设备控制面和查看者凭证
- viewer gate / gate_token 两级登录
- 查看者 profile 中的 tenant_key
- 独立长期 device_token
- browser/proxy/local/integration/media/tunnel 等并列凭证类型
- `?token=` 通用认证和媒体专用票据
- LAN 明文 HTTP 素材访问
- 请求体自报 source_device_id/device_id 作为身份依据
- Principal::Session 和只含 role 的 TunneledPeer
- 会话仅建立时校验 AccessKey 的授权方式
- 无主体/audience/grant_version/expiry 的持久化令牌
- 同一客户端、同一目标库按 LAN/WAN 分别签发访问密钥
- 库内 `.hawk/config.toml` 明文保存 AccessKey
- 本机插件通过发现端点取得完整 admin token

## 12. 非目标

- 用户账号、多人组织、角色继承、自定义 ACL
- 文件夹级分享和跨用户协作
- 公网网页版或在浏览器中实现另一套公网 E2EE 协议
- 以权限系统替代文件夹/分类/标签锁
- 隐藏控制面流量元数据或抵抗 server 拒绝服务
- 完全不可信的 Device 成员注册机构

不采用 JWT/OIDC：当前需要立即吊销、角色收窄、设备开关逐请求生效、Session audience 和资源绑定。有状态不透明 Session 更直接。未来接入外部账号时再评估 OIDC，本地授权层仍保留。

## 13. 实施顺序

1. 建立 Principal/Grant/Session/AccessContext 和授权不变量测试。
2. 重写 hawk-server 控制面：tenant/device 认证、设备签名挑战、资源授权和会话参与者检查。
3. 重写 daemon：统一 Session、逐请求 AccessKey 复核、target_identity、隧道租约和续期。
4. 将原生 LAN Peer 迁移到 iroh，统一 LAN/WAN profile 和共享 Rust 传输库。
5. 删除公网网页版与 server AccessKey 体系，完成 AccessKey-only rendezvous。
6. LAN Web 切换 HTTPS 和 Session Cookie，迁移凭证存储及本机 LocalIntegration。
7. 更新桌面、iOS、Android、插件及协议文档，删除全部旧路径并执行全量安全验收。

每一步以目标模型为准，不保留 V1 字段、回退认证或双协议分支。

## 14. 验收不变量

### 数据面零信任

- relay 和 hawk-server 日志、内存代理与持久化文件中不存在素材明文、搜索内容或 AccessKey。
- hawk-server 不存在 viewer/login、viewer token、ViewerHost 和素材代理路由。
- AccessKey 不发送给 hawk-server；server 不保存 AccessKey 验证子。
- server 替换寻址结果时，客户端因 target_identity 不匹配而拒绝连接。
- relay 只能转发密文，不能终结客户端与目标 daemon 的 iroh 会话。

### 凭证与资源隔离

- AccessKey Session 不能调用 tenant/device 控制面。
- 任一 Session 不能跨 audience 使用。
- Device A 不能以 Device B 发心跳、建 WSS、改资料或 accept/reject。
- 没有设备私钥时，仅持有 device_id、nonce 或旧 Device Session 不能重新认证。
- 跨 tenant 的设备、key、session 查询与操作一律返回 404。
- 非会话参与者不能 get/cancel；非 target 不能 accept/reject。
- 未认证 AccessKey 隧道只能调用登录握手，不能读取库信息或素材端点。
- 未声明 Action/Resource 的端点默认拒绝。

### 统一 AccessKey 与加密通道

- 同一 AccessKey 在 LAN/WAN 解析为同一 key_id 和目标库，不需要第二把用户凭证。
- 同一 editor AccessKey 在 LAN 可获得 editor，在 WAN 只能获得 importer。
- LAN/WAN 原生素材流量均为 iroh 端到端加密，不存在明文 HTTP 回退。
- LAN 浏览器只接受有效 HTTPS 和目标证书身份。
- 网络切换后旧 Channel Session 和权限快照不能复用。
- LAN 断开后，客户端能用同一 profile 自动完成 WAN 认证，无二次用户输入或审批。
- admin/Edit/Manage 不能经 WAN 获得。

### 会话与吊销

- AccessKey 删除后，已有 viewer/importer 隧道下一请求失败。
- AccessKey 降权后，已有会话下一请求按新角色执行。
- allow_import=false 后，已有 importer 下一 WAN 写请求失败。
- Device 吊销后新会话失败；取消事件丢失时，租约到期后请求失败。
- Session 过期、logout、key 删除后不能继续使用；logout 同时关闭目标会话。
- grant_version 陈旧时不能沿用旧角色。
- 缺少正确 source_endpoint_id 时，单独知道 session_id 不能通过隧道认证。

### 传输与风控

- `?token=` 不能认证任何 API；LAN 图片和 SSE 只能使用有效 Session Cookie 或 Bearer。
- LAN Cookie 必须带 Secure；Cookie 写请求缺少合法 Origin/自定义头时被拒绝。
- 本机发现不能经远程隧道使用，也不能返回 ProcessAdmin Session。
- 同机反代下不同外部客户端 IP 不共享同一个失败桶。
- 非受信来源提供的 Forwarded 系列头不被采信。
- 随机无效 key/device/nonce/endpoint 的状态记录不会无界增长。

## 15. 已确认决策

1. 用户只需理解 tenant_key 和 AccessKey 两种长期 key，其他身份材料由系统自动维护。
2. 设备复用 Ed25519/iroh 私钥签名认证，不再发放 device_token。
3. 所有短期认证状态统一为 session_token，以 audience 和 ResourceScope 隔离。
4. 同一客户端访问同一库时，LAN/WAN 共用 AccessKey 和 target_identity。
5. 原生客户端 LAN/WAN 全部端到端加密，LAN 不再使用明文 HTTP。
6. 取消公网网页版；hawk-server 不再终结素材隧道，只保留控制面、受限 rendezvous 和密文中继。
7. AccessKey 只由目标 daemon 验证，hawk-server 不保存 AccessKey 或验证子。
8. LAN 浏览器若保留，必须使用 HTTPS 和显式证书信任。
9. AccessKey 和设备会话采用本地逐请求复核 + 有界会话租约；不采用 JWT/OIDC。
10. 接受破坏性协议、存储、传输和客户端调整，不保留旧认证路径。

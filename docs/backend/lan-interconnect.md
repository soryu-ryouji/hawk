# 局域网 hawk-app 互联设计

桌面版 hawk 之间在同一局域网内直接互看素材库，不经过 hawk-server，也不需要先完成远程访问接入。本文描述发现、配对、连接与安全边界；广域网链路仍是独立的 [远程访问设计](remote-access.md)。

## 目标与非目标

**目标**

- 同一网段的两个 hawk-app 能发现彼此，用户在目标端确认配对请求后直接连接
- 连接后复用现有 `/api/v1/*` HTTP 契约：浏览、搜索、预览、缩略图、SSE 与已授权的写操作不新增数据通路
- 不要求用户配置 hawk-server、service-key 或设备身份；离线局域网可用
- 配对凭证不进入发现广播；默认只读，目标端开启写权限时才授予写能力

**非目标**

- 公网/跨 NAT 互联：由 remote + iroh 负责
- 共享库给不受信用户：配对请求确认是设备所有者现场授权，不是账号体系
- 跨网段自动发现：当前覆盖常见家用/办公 IPv4 LAN；VLAN、访客网络隔离与 IPv6 留给后续 mDNS/SRV 方案
- 移动端：iOS/Android 仍走既有 LAN token / QR 配对载荷，新增桌面互联协议不改变其契约

## 与远程访问的边界

| 能力 | 局域网互联 | 远程访问 |
| ---- | ---------- | -------- |
| 发现/匹配 | UDP 组播 + 广播，无需服务器 | hawk-server 设备列表 + WSS 信令 |
| 数据通路 | 目标 daemon 的 hawk-app 直连端口（HTTP API，默认 27375） | 本机 daemon 会话代理 → iroh QUIC → 目标 daemon |
| 鉴权 | 目标端审批后为该设备签发的 `[web].peer_keys` 直连密钥 | service-key + hawk-app key，两级验证 |
| 写能力 | 直连密钥自身的 `write`；与网站 `keys` 独立 | `remote.writable` + `remote_writer` 白名单 |
| NAT/公网 | 不支持（同网段） | iroh 打洞/中继 |

两条链路互不放开权限：开启局域网写不会放开广域网写，反之亦然。

## 总体架构

```text
设备 A（目标库）                          设备 B（浏览方）
┌──────────────────────┐                 ┌──────────────────────┐
│ hawk-app             │                 │ hawk-app             │
│  hawk-daemon :27371  │                 │  hawk-daemon :27371  │
│  peer API  :27375    │◄── HTTP ────────│  连接描述符指向 A 的  │
│  UDP discovery :27374│   直接请求      │  peer 地址 + token    │
└──────────────────────┘                 └──────────────────────┘
             ▲ UDP probe / announce（仅展示信息，不含 token）
             └──────────────────────────────────────────────────
```

源端 app 只是在主进程把「当前连接」从本地库切成 `http://<A-IP>:27375`，前端沿用同一条 `api_base + token` 渲染管线。**不需要第三进程、端口转发、会话代理或服务器。**

## 发现协议

- 端口：UDP `27374`
- 组播地址：`239.255.72.75`；同时向 `255.255.255.255:27374` 发送广播，兼容不转发组播的交换机/热点
- 消息为 JSON，`hawk: 1`，`op` 为 `probe` 或 `announce`
- 目标 daemon 在 hawk-app 直连端口就绪时应答；不需要预先存在 peer key，网站访问开关也独立
- 公告字段：稳定 `id`（主机名 + 库根路径的 BLAKE3 前缀）、设备名、库显示名、hawk-app 直连端口（`peer_port`）、`writable` 提示（按 peer key 汇总）
- **token 永不进入 UDP 包**。发现只解决「谁可以配对」，授权在配对阶段完成
- 源端 daemon 的 `GET /api/v1/app/lan/peers` 发起 probe，等待最多 900ms，按 id 去重后返回；本机自身从结果中过滤
- 广播/组播被访客网络隔离时，源端 UI 提供手动地址兜底，仍走同一自动配对请求

稳定 id 仅用于展示去重和配对记录，不承担密码学身份。UDP 明文可被同网段伪造，因此不能作为授权依据；真正的授权凭证是目标端审批与 token。

## 配对流程

```text
A 打开 设置 → 网络 → 局域网互联 → 扫描设备
A 选择目标 B，点击「申请配对」
  → POST B:27375/api/v1/app/lan/pair-requests  (免 Bearer，仅直连端口)
      { client_name, writable }
B 的 hawk 主界面弹出「局域网配对请求」
  → 拒绝 / 仅读取 / 可读写
  → POST /api/v1/app/lan/pair-requests/{id}/decision  (本机 admin)
B daemon 为 A 创建一把专用 peer key
A 轮询 GET /api/v1/app/lan/pair-requests/{id}
  → 返回 { token, writable, peer_id, name, library_name }
A 校验目标 app/info 后保存连接并切换
```

实现要点：

- 请求 id 为随机高熵值，90 秒过期；待确认请求数量有上限，重复点击会复用同一请求
- 目标端确认时动态创建该设备专用 peer key，不需要预先创建密钥或生成配对码
- 目标端可随时把该设备切换为仅读取/可读写，或删除设备撤销共享
- 权限变更广播 lan.access.changed，源端收到后重新读取 app/info 热刷新界面
- 旧的 8 位配对码流程已移除；移动端仍通过二维码携带 token 授权
## 连接与状态管理

主进程的连接优先级：

```text
remoteConn（广域网会话代理）
  > lanConn（局域网目标 LAN web）
  > localDescriptor（本机 daemon）
```

- `kind` 字段区分 `local | remote | lan`；`remote` 布尔保留为「非本机」语义，旧展示逻辑据此兜底
- 切换时前端收到 `connection-changed`，执行统一的 abort 请求/SSE → 清 store → `GET /app/info` → 重注水
- 已配对设备保存在 `~/.config/hawk/config.toml` 的 `lanPeers`（含 URL 与 token，渲染层只拿脱敏 profile）
- `lanTarget` 记录上次浏览的局域网设备；daemon 就绪后先尝试恢复，失败回退本地库
- 主进程每 5 秒对当前目标 `GET /app/info` 健康检查；连续两次失败视为连接死亡，无声回切本地库并清除自动恢复目标（记录仍保留，可手动重连）
- 显式断开、切本地库或选择其它连接时清理 `lanTarget`

## UI

- **源端**：局域网互联 → 扫描设备 → 申请配对；等待目标端确认后自动连接
- **目标端**：主界面全局弹出配对请求，选择拒绝/仅读取/可读写；已授权设备可在设置中调整权限或删除
- **移动端**：目标端生成授权二维码，扫码客户端携带候选地址与 token 直连
- **当前连接**：局域网互联顶部显示当前设备，并从发现列表排除；支持测速与断开
- **无本地库模式**：远程查看屏同样提供局域网互联块
- **侧栏库名下拉**：已配对局域网设备与远程设备分区展示
## 安全模型

| 环节 | 措施 |
| ---- | ---- |
| 发现广播 | 不含 token/密钥；仅局域网可见的展示元数据 |
| 配对准入 | 目标端用户现场确认请求；请求 id 随机、90 秒过期，待确认数量有上限 |
| 访问凭证 | 每个授权设备独立 peer key，权限可动态调整为仅读取/可读写，删除即撤销；与网站 keys 分离 |
| 网络边界 | 仅局域网直连；不把直连 token 上传到 hawk-server；二维码可携带同一目标的多个候选地址，客户端自动择优 |
| 传输 | 明文 HTTP（与现有局域网 web 查看一致）；只建议在可信网络使用，公网必须走 HTTPS 远程链路 |
| 本地存储 | token 存在应用级 `config.toml`（Unix 下 0600）；渲染层不持久化 token |
| 权限隔离 | 局域网写、广域网写、配对写请求三套开关独立，不互相提权 |

后续可增强：mDNS/Bonjour SRV+TXT 替代 UDP 广播、配对阶段加设备公钥绑定、可选 HTTPS（自签证书指纹钉扎）。当前不引入这些复杂度的原因是：真实需求是同一可信 LAN 内快速互通，HTTP + 现有 token 语义已经与浏览器查看器同等级。

## 实现落点

| 位置 | 内容 |
| ---- | ---- |
| hawk-daemon/src/api/lan.rs | 双监听 supervisor、UDP 发现、自动配对请求/审批、按设备创建 peer key、权限变更事件 |
| `hawk-daemon/src/bootstrap.rs` | 启动发现响应器任务 |
| `hawk-daemon/src/api/mod.rs` | 配对端点免 Bearer 例外（仍受一次性码保护） |
| `hawk-app/electron/src/remote.ts` | 源端发现/配对/连接、连接描述符、健康检查、冷启动恢复 |
| `hawk-app/electron/src/app-config.ts` | `lanPeers` / `lanTarget` 持久化 |
| `hawk-app/web/src/domains/lan/` | 源端发现与配对状态 |
| `hawk-app/web/src/domains/settings/components/SettingsLanConnect.vue` | 局域网互联 UI |
| hawk-app/web/src/domains/settings/components/SettingsLanPairActions.vue | 已授权设备权限管理 + 移动端授权二维码 |

## 验证清单

- 两台 hawk-app 同网段：源端扫描到目标 → 申请配对 → 目标端选择权限 → 自动连接
- 重复确认/过期请求返回 409/410；目标端删除设备后源端请求返回 401
- 目标只有只读密钥时源端处于只读；有可写密钥并可匹配时能执行写操作
- 目标退出/断网：源端 10 秒内无声回本地库；重启源端会尝试恢复上次局域网目标
- 无本地库的远程查看模式同样能完成局域网配对
- 不配置 hawk-server、断网（仅局域网）时全流程可用

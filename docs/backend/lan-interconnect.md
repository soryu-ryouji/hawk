# 局域网 hawk-app 互联

桌面版 hawk 在同一局域网内直接互看素材库：不经服务器、不做传输加密，目标 daemon 的 Peer 端口直接提供
`/api/v1/*`。配对凭证与授权由目标端签发和执法。

## 目标与非目标

**目标**

- 同一网段的两个 hawk-app 能发现彼此，用户在目标端确认配对请求后直接连接
- 连接后复用现有 HTTP 契约：浏览、搜索、预览、缩略图、SSE 与已授权的写操作
- 不要求用户配置服务器、账号或证书；离线局域网可用
- 配对凭证不进入发现广播；默认只读，目标端开启写权限时才授予写能力

**非目标**

- 公网/跨 NAT 互联：用户可以自行用 Tailscale 等组网工具把设备放进同一虚拟局域网
- 共享库给不受信用户：配对请求确认是设备所有者现场授权，不是账号体系
- 跨网段自动发现：当前覆盖常见家用/办公 IPv4 LAN

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

源端 app 只是在主进程把「当前连接」从本地库切成 `http://<A-IP>:27375`，前端沿用同一条
`api_base + token` 渲染管线。不需要第三进程、端口转发、会话代理、隧道或服务器。

## 发现协议

- 端口：UDP `27374`
- 广播地址：`255.255.255.255` 与各网卡广播地址
- 消息为 JSON，`hawk: 1`，`op` 为 `probe` 或 `announce`
- 目标 daemon 在 Peer 端口就绪时应答；不需要预先存在 peer key，网站访问开关也独立
- 公告字段：稳定 `peer_id`、设备名、库显示名、Peer 端口、`writable` 提示；**不含任何 token**
- 源端 `GET /api/v1/app/lan/peers` 发起 probe，等待最多 900ms，按 id 去重后返回
- 广播被访客网络隔离时，源端 UI 提供手动地址兜底：输入目标地址后走自动配对审批，或输入预创建密钥直接连接

UDP 明文可被同网段伪造，因此不能作为授权依据；真正的授权凭证是目标端审批与 AccessKey。

## 配对流程

1. A 在「局域网互联」扫描到 B，点击「申请配对」。
2. A 有本地库时先在本机创建一把与 B 同名的反向设备密钥（默认只读），然后把回连 profile 放进请求：
   `POST /api/v1/app/lan/pair-requests`

   ```json
   {
     "client_name": "A",
     "role": "editor",
     "reverse": {
       "peer_id": "...",
       "name": "A",
       "library_name": "A 的素材库",
       "urls": ["http://192.168.1.10:27375"],
       "peer_key": "<A 为 B 签发的 peer key>",
       "role": "viewer",
       "target_library_id": "..."
     }
   }
   ```

3. B 的主界面弹出全局配对请求，用户选择拒绝、仅读取或可读写。
4. B 调用本机 admin 端点 `POST /api/v1/app/lan/pair-requests/{id}/decision`：
   - 允许时为 A 创建一把专用 peer key；
   - 双向请求除 `status=approved` 外还返回 `peer`（A 的回连 profile），B 主进程据此保存 A；
   - 单向请求 `peer=null`。
5. A 轮询 `GET /api/v1/app/lan/pair-requests/{id}`，校验 `protocol=3`，取得目标 token 后保存 B 并直连。
6. 双方都有本地库时，两端此后都拥有对方的连接 profile，无需再次申请配对。

协议要点：

- 请求 id 为随机高熵值，90 秒过期；待确认请求数量上限 8，重复点击会新增请求，客户端应避免连点。
- 目标端确认后通过独立设备密钥面创建 peer key，不需要预先创建网站密钥或生成配对码。
- 目标端可随时把设备切换为仅读取/可读写，或删除设备撤销共享；权限变更广播 `lan.access.changed`。
- 预创建连接密钥是目标端主动授权：源端仅需地址和密钥即可建立直连；密钥泄露风险由目标端设备列表收敛。
- 移动端仍通过二维码携带 token 授权，不走桌面配对协议。

## 连接与状态管理

- 源端 `lanConn` 直接指向 `http://<目标>:<peer_port>`，Bearer 目标 AccessKey。
- 主进程连接优先级：`lanConn` > 本地库；`kind` 区分 `local | lan`。
- 切换时前端收到 `connection-changed`，执行统一的 abort 请求/SSE → 清 store → `GET /app/info` → 重注水。
- 已配对设备保存在 `~/.config/hawk/config.toml` 的 `lanPeers`（含 URL 与 token，渲染层只拿脱敏 profile）。
- 同一目标换网段/换 IP 后，源端先用已保存 AccessKey 探测 UDP 发现到的同一 `peer_id` 新地址；验证通过就只更新 URL，不重新生成 key、不要求目标端重复确认。只有 key 失效或地址不可达时才回退到完整配对。
- 主进程每 10 秒对当前目标 `GET /app/info` 健康检查；失败后保持 `lanConn`/`lanTarget` 并进入恢复等待，继续每 10 秒探测；成功后自动重新注水，不静默回切本地库。只有用户显式“打开本地素材库”或忘记设备才清目标。
- 目标端 `/api/v1/app/lan/clients` 按 peer key 最近活跃时间显示在线设备（30 秒窗口）。

## UI

- **源端**：局域网互联 → 扫描设备 → 申请配对；等待目标端确认后自动连接
- **目标端**：主界面全局弹出配对请求，选择拒绝/仅读取/可读写；已授权设备可在设置中调整权限或删除
- **移动端**：目标端生成授权二维码，扫码客户端携带候选地址与 token 直连
- **当前连接**：局域网互联顶部显示当前设备，并从发现列表排除；支持测速与断开
- **侧栏库名下拉**：已配对局域网设备与本地历史库分区展示

## 安全边界

| 环节 | 措施 |
| ---- | ---- |
| 发现广播 | 不含 token/密钥；仅局域网可见的展示元数据 |
| 配对准入 | 目标端用户现场确认请求；请求 id 随机、90 秒过期，待确认数量上限 |
| 访问凭证 | 每个授权设备独立 peer key，权限可动态调整，删除即撤销 |
| 网络边界 | 仅局域网直连，不做传输加密；跨网段由用户自行组网 |
| 本地存储 | token 存在应用级 `config.toml`（Unix 下 0600）；渲染层不持久化 token |
| 权限隔离 | 局域网最高 editor，不能修改 daemon 配置或密钥 |

锁（文件夹/分类/标签）仍是 daemon 强制的可见性过滤，与传输无关。

## 实现落点

| 位置 | 内容 |
| ---- | ---- |
| `hawk-daemon/internal/api/lan.go` | 双监听 supervisor、UDP 发现、配对请求、设备密钥、在线状态 |
| `hawk-daemon/internal/auth/auth.go` | AccessKey / Web Session / 集成 Session |
| `hawk-app/electron/src/connections.ts` | 源端发现、双向/单向配对、回连 profile、连接描述符、健康检查 |
| `hawk-app/electron/src/app-config.ts` | `lanPeers` / `lanTarget` 持久化 |
| `hawk-app/web/src/domains/lan/` | 源端发现、已配对设备与配对状态 |
| `hawk-app/web/src/domains/settings/components/SettingsLanConnect.vue` | 局域网互联 UI |
| `hawk-app/web/src/domains/settings/components/SettingsLanDevices.vue` | 已授权设备权限管理 + 移动端二维码 |

## 验证清单

- 两台 hawk-app 同网段：源端扫描到目标 → 申请配对 → 目标端选择权限 → 自动连接
- 重复确认返回 409；轮询已过期请求返回 `status=expired`；目标端删除设备后源端请求返回 401
- 目标端审批「仅读取」时源端只读；审批「可读写」或后续切换后可执行写操作
- 目标退出/断网：源端进入恢复等待界面并每 10 秒重试；恢复后自动回到目标库；点“打开本地素材库”才切回本地
- 手机扫码 / 地址+密钥两条路径都能直连目标 peer 端口

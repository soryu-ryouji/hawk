# 远程访问设计

> 远程访问是可选能力：每台设备两个进程——hawk-app（Electron 壳）与 hawk-daemon（素材库服务），广域网连接能力全部收在 daemon 的 **`remote/` 模块**（可选子系统，supervisor 管理）。远程访问服务端 **hawk-server** 位于本仓库 `hawk-server/`，单用户方案，已实现 P1 控制面。接口契约见 [remote-protocol.md](remote-protocol.md)。

## 目标与非目标

**目标**：在客户端配置接入信息（服务端地址 + 访问密钥）后，查看自己其他在线设备，远程浏览对方设备上的素材库（只读）。

**非目标**（v1 不做）：

- 远程写入素材库（修改标签、导入素材等）——远程一律只读，写入仍在本机完成
- 多用户 / 会员体系——服务端为单用户自托管方案，准入即一把固定 access key
- 共享库给他人——设备只属于同一套接入凭据
- 移动端查看器——后续迭代，协议不为此预留特殊设计

## 总体架构

两台设备各自的 hawk-daemon 内嵌 remote 模块，彼此之间建立端到端 QUIC 隧道，素材数据始终由「素材库所在机」的 daemon 产出。

```text
        设备 A（素材库所在机）                      设备 B（查看方）
┌────────────────────────────┐          ┌────────────────────────────┐
│ hawk-app (Electron)        │          │ hawk-app (Electron)        │
│  前端 ──HTTP/SSE──► daemon  │          │  前端 ──HTTP──► 会话代理     │
│  主进程：拉起/回收 daemon    │          │  主进程：拉起/回收 daemon    │
└──────────┬─────────────────┘          └──────────┬─────────────────┘
           ▼                                       ▼
┌────────────────────────────┐          ┌────────────────────────────┐
│ hawk-daemon                │          │ hawk-daemon                │
│  API 127.0.0.1:p1          │          │  API 127.0.0.1:p1          │
│  ┌──────────────────────┐  │          │  ┌──────────────────────┐  │
│  │ remote/ 模块          │  │          │  │ remote/ 模块          │  │
│  │ 信令（注册/心跳/WSS）  │  │          │  │ 信令                  │  │
│  │ A 侧隧道端 ◄───────────┼──┼─QUIC 隧道（端到端加密）──┤ B 侧会话代理 :p4 │  │
│  │ （进程内转发 viewer 级）│  │          │  │ 127.0.0.1，随机 token │  │
│  └──────────────────────┘  │          │  └──────────────────────┘  │
└───────────────┬────────────┘          └───────────────┬────────────┘
                │  HTTPS/WSS 控制面（注册/心跳/会话/WSS）  │
                └───────────────┬───────────────────────┘
                                ▼
              hawk-server（本仓库 hawk-server/，P1 控制面已实现；
              中继为二期，见 remote-protocol.md）
```

端口默认值：hawk-server 27370 / daemon API 27371 / LAN web 27372 / QUIC UDP 27373 / B 侧会话代理 27374。本地端口由 daemon 在会话建立时分配空闲端口（不固定绑定）。

## 组件职责

### hawk-daemon（含 remote/ 模块）

素材库服务本体 + `remote/` 模块承载全部广域网能力：

- 信令客户端：设备注册、周期心跳（30s）、WSS 控制通道（自动重连）、设备列表、会话发起与响应
- 可达性：UPnP/NAT-PMP 端口映射（igd）、公网 IPv4 / 全局 IPv6 探测上报（阶段 1b）
- 数据面（阶段 1b）：QUIC 隧道端与 B 侧会话代理
  - A 侧隧道端：隧道内 HTTP 请求**在进程内转发**给自身路由，携带 viewer 级权限标记（见安全模型）
  - B 侧会话代理：环回监听器（supervisor 管理，同 LAN web 模型），每次会话随机 token，web 查看器以 `?token=` 携带，响应不带 CORS 头
- 状态与事件：经 daemon 现有 REST（`/api/v1/remote/*`）与 SSE 事件暴露给 app 前端与主进程

**可选子系统约束**：`[remote]` 配置段 `enabled = true` 且 url/key 非空才激活，supervisor reconcile 模型（同 LAN web）——配置变更热重启信令；绑定/连接失败只写状态不崩进程。未启用时任务不 spawn、端口不绑定，该能力在运行时不存在。

### hawk-app（Electron 壳，接线枢纽）

主进程职责不变（拉起/回收 daemon、注入 admin token），远程相关只做两件事：

- 维护「当前活动连接」（见连接模型），向渲染进程下发连接变更；远程库的 `api_base` 指向 daemon 的会话代理端口
- 订阅 daemon SSE 的 remote 会话生命周期事件，驱动断线回切

不再拉起第三进程，无端口预选下发、无受托 token 注入、无额外状态 API 通道。

### hawk-server（本仓库 `hawk-server/`，单用户）

远程访问服务端，与 hawk-daemon **零代码共享**（独立 crate，协议契约是唯一共享物）：

- 单用户准入：启动时经 `--access-key` / `HAWK_SERVER_ACCESS_KEY` 配置一把固定访问密钥；无账号、会员、计费、额度概念
- P1 控制面（已实现）：设备注册/心跳/列表/更新/注销、WSS 控制通道、会话编排（直连候选组装、状态机）
- 持久化：设备注册表 JSON 文件；部署：监听明文 HTTP（默认 27370），TLS/WSS 由前置反代终结
- P2 中继（二期）：服务端内加密字节流转发，协议未定

## 数据流：daemon 为何不需要理解「远程」语义

remote 模块是「网络延长线」，只做字节转发，不产生、不转换任何素材数据。隧道内 HTTP 请求与局域网浏览器请求走同一条路由管线：

```text
B 浏览器 / B app 查看窗口
  │ ① 普通 HTTP 请求
  ▼
B 侧会话代理（remote 模块）──② 原样塞进 QUIC 隧道──►
A 侧隧道端（remote 模块）──③ 进程内转发，标记 viewer 级权限──►
A 的 API 路由 ──④ 与局域网请求无区别地处理──► 响应原路返回
```

- B 侧的素材库服务与远程查看完全无关（照常服务 B 自己的库）；跨公网传输的请求/响应，收件方是浏览器
- app 前端查看远端库不需要新数据通路：app 的窗口与 web 查看器是同一个构建产物，只是 API base 指向会话代理端口

## 连接模型：切换库无感

设计目标：用户在「我的库」与「远端设备库」之间切换时，UI 结构与连接方式对前端无感——前端始终只面对「一个 API base + 一个 token」。

**连接描述符**（主进程维护，preload 暴露 `getCurrent()` + `onChanged()`）：

```text
Connection {
  api_base: "http://127.0.0.1:<port>"   // 端口语义前端不可见
  token:    "..."
  label:    "我的库" | "家里台式机"        // 仅用于展示
}
```

- 本地库：`api_base` 指向 daemon API 端口；远程库：指向 B 侧会话代理端口
- 前端收到 `connection-changed` 后执行**统一的切换重置**：abort 进行中的请求与 SSE → 清空内存 store → `GET /app/info` → 按返回重新注水。该流程是启动注水的泛化，只写一次
- **断线自动回切**：隧道断开、对端离线时，主进程经 daemon SSE 的 remote 事件获知，自动把当前连接切回本地库再通知前端。用户看到的是无声回到自己的库，而非远程白屏报错
- **无感的边界**：只读降级与来源标识保留。远程库经 `app/info` 的 `access`（viewer 级）自然触发现有只读降级，前端无专门分支；窗口保留「当前浏览：家里台式机」轻标识——这是安全与认知底线，不是可省略的区分逻辑

## 关键设计决策

**1. remote 是 daemon 内模块，不是独立进程**

原方案（独立进程 hawk-remote）的两个动机在当前前提下已失效或可用更省的方式满足：闭源云端边界（社区构建物理剔除云端代码）随服务端开源自托管而消失；崩溃域隔离在 Rust 里可用 supervisor 任务级重启达到同等实效——daemon 已有先例：LAN web 就是 reconcile 模型的可选网络子系统，故障只写状态不崩进程。合并后省掉三块接线：第三进程的 spawn/传参/回收、受托 token 双进程注入机制、独立状态 API 端口（设备列表/会话事件走 daemon 现有 REST + SSE）。代价是 quinn/igd 等依赖进入 daemon 依赖树（未启用时运行成本为零），以及 WAN 代码与素材库服务同进程（以 supervisor + `[remote]` 可关闭缓解）。

**2. 直连优先，服务器只做撮合**

素材浏览是重带宽场景，自托管服务器的带宽/流量有限，数据面默认不经过服务器：服务端只做注册、在线与候选撮合（控制面），素材数据走设备间 QUIC 直连。中继（P2）仅作为直连失败时经用户确认的兜底，消耗的是用户自己的服务器带宽。

**3. 数据面从第一天用 QUIC（quinn）**

QUIC 自带多路复用、TLS 1.3 端到端加密、抗 NAT 重绑定，且为 P3 打洞（免端口映射 P2P）预留了传输基础。

**4. 协议契约先行，零代码共享**

客户端（daemon remote 模块）与 hawk-server 之间的契约是 remote-protocol.md。hawk-server 保持独立 crate、不建 workspace 羁绊、不共享代码——它对 hawk 的全部知识就是协议文档；remote 模块对 daemon 本体的全部知识就是进程内路由 + viewer 权限标记。

**5. 客户端最小知识：URL + KEY，无登录**

客户端配置只有服务端地址 URL 与访问密钥 KEY（`[remote]` 配置段）。协议无登录端点、无会话令牌；key 无效即 401，客户端直接展示服务端返回的 message。

**6. 会话认证在 QUIC 层，隧道内 HTTP 进程内授权**

设备自签名证书指纹在服务端登记（TOFU），会话建立时经控制面交换指纹，QUIC 对端以指纹钉扎验证。隧道内 HTTP 由 A 侧隧道端在进程内标记 viewer 级权限（与 LAN viewer 同级只读）——查看方 B 全程不接触 daemon 的任何 token，服务端也接触不到。

**7. 中继需用户主动开启**

连接顺序：IPv4 直连（UPnP）→ IPv6 直连 → 询问用户是否中继。中继消耗自己的服务器带宽，任何环节都不自动回落——「中继回退」是客户端设置项（默认关闭），开启后直连失败时弹确认，用户同意才建立中继会话。IPv6 直连是免映射、不经服务器的 P2P 路径；候选可达性由客户端实际连接尝试判定，服务端只负责收集上报、不下结论。

## 连接建立流程

### 直连模式（阶段 1）

```text
B 配置接入（config.toml [remote] url + key）→ 拉取设备列表（A 在线 + 心跳携带可达性信息）
B 前端发起会话（mode=direct）
hawk-server：A 心跳上报过可达候选（IPv4 UPnP 映射 / 全局 IPv6）？
  ├─ 是 → 创建会话，经 WSS 推送给 A：session.request
  │        A 侧自动接受（设备级开关 allow_remote 控制）→ 回 accept
  │        服务端向双方推送 session.update(ready)：候选列表（IPv4 映射优先、IPv6 次之）+ 指纹
  │        B 依序尝试候选（单候选超时 5s），任一成功即建立 QUIC 连接（指纹钉扎）
  │        隧道就绪 → 主进程切换连接描述符 → B 前端无感重置为远端库
  └─ 无任何候选 → 会话状态 unreachable
```

直连失败（候选全部超时或无候选）时不自动回落中继：「中继回退」开关（默认关闭）已开启时，弹窗询问用户是否经中继继续（消耗自己的服务器带宽），确认后转入中继流程；未开启则提示「当前网络无法直连；如需经中继访问，可在设置中开启」。

### 中继模式（阶段 2，协议未定）

单用户方案下中继大幅简化：无票据、配额、用量上报体系，服务端作为加密字节流的转发点（准入即控制面同一把 access key），QUIC 仍在 A↔B 之间端到端建立，服务端看不到素材内容。细节二期设计。

## 安全模型

| 环节     | 措施 |
| -------- | ---- |
| 控制面   | HTTPS/WSS（TLS 由前置反代终结）；access_key 由部署者生成配置（建议 256-bit 随机值），服务端恒定时间比较 |
| 数据面   | QUIC TLS 1.3 端到端加密，证书指纹经控制面交换 + 钉扎；服务端接触不到素材内容（直连不过服务端，中继只搬运密文） |
| 隧道端点 | 隧道内请求在进程内转发时标记 viewer 级权限（只读，与 LAN viewer 同级），且不经环回监听端口——`GET /api/v1/app/token`（免鉴权返回主 token，Host 限定环回）对隧道流量天然不可达，无需 denylist 特判 |
| 本地防护 | B 侧会话代理：每次会话随机 token、响应无 CORS、环回监听 |
| 凭据存储 | 接入配置（`[remote]` 的 url + key）存 `config.toml`——与 `[web]` token 同级先例，明文但随库同步（配一次全设备生效，单用户场景是特性）；设备身份（device_id、自签证书私钥）存 `.hawk/remote/`（库级，本地专用目录，不参与同步） |

## 分发形态

- hawk-server 位于本仓库 `hawk-server/`，随 AGPL 开源，自托管部署（单二进制 + CLI/env 配置，TLS 交反代）
- daemon 单二进制内含 remote 模块，无第三进程、无构建期门控；`[remote]` 未启用则运行时该能力不存在（任务不 spawn、端口不绑定）
- app 远程 UI 以 daemon `/api/v1/remote/*` 状态为准：未配置时展示接入引导，无独立构建开关

## 对 hawk 仓库的改动清单

| 位置                       | 改动 |
| -------------------------- | ---- |
| `hawk-server/`             | 远程访问服务端（已实现 P1 控制面：设备/心跳/WSS/会话撮合；P2 中继二期） |
| `hawk-daemon/src/remote/`  | 新模块：信令客户端 / 可达性 / QUIC 隧道端 / B 侧会话代理，supervisor reconcile 管理 |
| `hawk-daemon` config       | `[remote]` 配置段（enabled/url/key），保存即热生效（同 `[web]`） |
| `hawk-daemon` 鉴权中间件   | 进程内 viewer 级权限标记（供隧道转发请求使用），无 remote 语义、无门控 |
| `hawk-daemon` API          | `/api/v1/remote/*` 状态端点 + SSE remote 会话生命周期事件 |
| `hawk-app`（主进程 + 前端） | 远程 UI（接入配置/设备列表/会话流程）；连接描述符与 `connection-changed`；订阅 daemon SSE remote 事件驱动断线回切 |
| `.hawk/remote/`            | 本地专用设备身份目录（device_id、自签证书私钥），不参与同步 |

## 分级落地计划

| 阶段 | 内容 | 验收标准 |
| ---- | ---- | -------- |
| 1a   | ~~hawk-server P1 控制面~~（已完成）+ daemon remote/ 信令：`[remote]` 配置、设备身份（device_id + 自签证书）、注册/心跳/WSS 自动重连、状态 API | 设备配置后可注册上线、维持在线、接收并自动响应会话请求 |
| 1b   | 可达性（UPnP/IPv6）+ QUIC 数据面（A 侧隧道端/B 侧会话代理）+ app 远程 UI 与连接描述符 | 有公网 IPv4（UPnP）或全局 IPv6 的设备端到端远程查看可用；切换本地/远端库无感；断线自动回切 |
| 2    | 服务端中继 + 客户端中继逻辑 +「中继回退」开关与确认弹窗 | 直连失败经用户确认走中继；QUIC 端到端语义不变（服务端只见密文） |
| 3    | QUIC 打洞（免端口映射 P2P，心跳已预留 `nat_type`） | 视阶段 1 直连成功率数据决定是否投入 |

## remote/ 模块结构（规划）

```text
hawk-daemon/src/remote/
├── mod.rs          # supervisor（reconcile：[remote] 配置变更 → 热重启信令）+ 状态快照
├── settings.rs     # [remote] 配置段（enabled/url/key）
├── identity.rs     # .hawk/remote/identity.json（device_id、rcgen 自签证书与指纹）
├── signal/         # REST 客户端（注册/心跳/设备/会话）+ WSS 通道 + 自动重连
├── reachability/   # UPnP/NAT-PMP 映射维护、公网 v4/v6 探测（1b）
├── session/        # 会话编排、候选依序尝试（单候选 5s）（1b）
└── data/           # quinn 端点、A 侧隧道端（进程内转发 viewer 级）、B 侧会话代理（1b）
```

新增依赖：tokio-tungstenite（WSS）、reqwest（REST，rustls）、rcgen + sha2（自签证书与指纹）；1b 增加 quinn、igd。

## 相关文档

- [远程访问协议](remote-protocol.md)（接口契约，唯一共享物）
- [hawk-app 设计](../frontend/hawk-app.md)（进程模型、打包分发）

# 远程访问设计

> 远程访问是可选能力：每台设备两个进程——hawk-app（Electron 壳）与 hawk-daemon（素材库服务），广域网连接能力全部收在 daemon 的 **`remote/` 模块**（可选子系统，supervisor 管理）。远程访问服务端 **hawk-server** 位于本仓库 `hawk-server/`，多租户方案（池 = 租户），已实现控制面、中继、网页版与受限广域网写。接口契约见 [remote-protocol.md](remote-protocol.md)。

## 目标与非目标

**目标**：在客户端配置接入信息（服务端地址 + service-key）后，查看自己其他在线设备，远程浏览对方设备上的素材库；持有带写权限 hawk-app key 的浏览器插件可向开启了广域网写入的设备导入素材。外部查看者（网页版/插件）需两级凭证：service-key（池门禁）+ hawk-app key（逐把读/写权限）。

**非目标**（v1 不做）：

- 广域网完整库管理（删除、改标签/评分、管理设置等）——v1 只开放插件保存所需的 `item/add` 与 `folder/create`
- 账号 / 会员体系——服务端为多租户自托管方案，准入即租户 service-key（后续可接账号系统，届时 service-key 下沉为内部凭证，协议不动）
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

端口默认值：hawk-server 9100 / daemon API 27371 / LAN web 27372 / QUIC UDP 27373 / B 侧会话代理 27374。本地端口由 daemon 在会话建立时分配空闲端口（不固定绑定）。

## 组件职责

### hawk-daemon（含 remote/ 模块）

素材库服务本体 + `remote/` 模块承载全部广域网能力：

- 信令客户端：设备注册、周期心跳（30s，携带当前库名与可达性）、WSS 控制通道（自动重连）、设备列表、会话发起与响应
- 可达性：UPnP/NAT-PMP 端口映射（igd；期望端口被占时退到网关分配的任意外部端口并上报实际值）、公网 IPv4 / 全局 IPv6 探测上报（IPv6 探测独立于服务端地址族：UDP connect 知名全球 IPv6 仅做路由决策）（阶段 1b）
- 数据面（阶段 1b）：QUIC 隧道端与 B 侧会话代理
  - A 侧隧道端：隧道内 HTTP 请求**在进程内转发**给自身路由，携带 viewer 级或受限远程写权限标记（见安全模型）
  - B 侧会话代理：环回监听器（supervisor 管理，同 LAN web 模型），每次会话随机 token，web 查看器以 `?token=` 携带，响应不带 CORS 头
- 状态与事件：经 daemon 现有 REST（`/api/v1/remote/*`）与 SSE 事件暴露给 app 前端与主进程

**可选子系统约束**：应用级 `remote.toml` 的 `enabled = true` 且 url/key 非空才激活，supervisor reconcile 模型（同 LAN web）——接入参数变化热重启信令，`writable` 变更热生效不重启；绑定/连接失败只写状态不崩进程。未启用时任务不 spawn、端口不绑定，该能力在运行时不存在。热重启时旧数据面（QUIC 端点 + A 侧 accept 循环）随任务一起关停，supervisor 等其结束再拉起新任务，并对端口绑定做短暂重试——避免新一轮绑定自己刚占用的端口；停用后端口立即释放。

**应用级配置与设备身份**：接入配置存 `~/.config/hawk/remote.toml`（Electron 经 `--remote-config` 传入），设备身份存同级的 `remote/identity.json`。两者不再随素材库切换：同一台机器在服务端始终是一台设备，切换库后心跳把设备表的 `library_name` 收敛到当前库。独立运行 daemon 时不传 `--remote-config`，回退到库内 `.hawk/remote.toml` 与 `.hawk/remote/`（测试/单库脚本用）。手工修改 `remote.toml` 需重启 daemon；设置面板经 API 写入即时生效。

```toml
# ~/.config/hawk/remote.toml（应用级；设备身份与访问密钥在 ~/.config/hawk/remote/）
enabled = true
url = "https://hawk.example.com"
service_key = "hs_<service-key>"
quic_port = 27373
writable = false
```

访问密钥（hawk-app key）存同级 `remote/keys.json`（sha256 验证子 + 标签 + 权限，0600；明文不落盘）。签发/改权限/吊销走 daemon API（`remote/keys` 端点），变更与每次接入时全量推送 server 映射。

### hawk-app（Electron 壳，接线枢纽）

主进程职责不变（拉起/回收 daemon、注入 admin token），远程相关只做两件事：

- 维护「当前活动连接」（见连接模型），向渲染进程下发连接变更；远程库的 `api_base` 指向 daemon 的会话代理端口
- 无本地素材库时提供「仅浏览远程素材库」入口：拉起无库形态的 daemon（远程查看模式，不创建任何本地目录），前端进远程查看屏
- 订阅 daemon SSE 的 remote 会话生命周期事件，驱动断线回切

不再拉起第三进程，无端口预选下发、无受托 token 注入、无额外状态 API 通道。

### hawk-server（本仓库 `hawk-server/`，多租户）

远程访问服务端，与 hawk-daemon **零代码共享**（独立 crate，协议契约是唯一共享物）：

- 多租户准入：`tenant add/list/revoke` CLI 管理 service-key（程序生成 256-bit 随机、明文只打印一次、服务端只存 HMAC 哈希）；池间设备/会话/key 注册表完全隔离；无账号、会员、计费、额度概念；凭证验证面带 IP + 租户双维度风控（详见协议「凭证模型」）
- 控制面（已实现）：设备注册/心跳/列表/更新/注销、WSS 控制通道、会话编排（直连候选组装、打洞编排、状态机）
- 打洞探测口（已实现，P3）：UDP 9101 极简 STUN 式应答，观测会话双方的公网 endpoint（响应 ≤ 请求 + token 校验 + IP 限速，防反射放大）
- 中继（已实现）：`GET /api/v1/relay` WS 配对转发，QUIC 端到端语义不变（只见密文）
- **网页版宿主（已实现）**：服务端作为 B 侧（以「网页版」全局设备身份走既有协议发起 relay 会话、经进程内中继 socket 建立 QUIC 并终结隧道），任意浏览器经域名浏览素材库。浏览器令牌（bt）由两级登录换发（service-key → 门令牌 → hawk-app key → bt；bt 绑定唯一目标设备与 read/write 作用域，服务端只存哈希），查看面 API（`/api/v1/viewer/*`）+ 隧道代理 + 登录页；查看器前端产物不落服务端，`/app` 与 `/assets/*` 同样经隧道由目标设备 daemon 托管。部署见 [远程访问部署](remote-deployment.md)
- 持久化：设备注册表/网页版身份/令牌 JSON 文件；部署：监听明文 HTTP（默认 9100），TLS/WSS 由前置反代终结

## 数据流：daemon 为何不需要理解「远程」语义

remote 模块是「网络延长线」，只做字节转发，不产生、不转换任何素材数据。隧道内 HTTP 请求与局域网浏览器请求走同一条路由管线：

```text
B 浏览器 / B app 查看窗口
  │ ① 普通 HTTP 请求
  ▼
B 侧会话代理（remote 模块）──② 原样塞进 QUIC 隧道──►
A 侧隧道端（remote 模块）──③ 进程内转发，按会话标记 viewer 级/受限写权限──►
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
- **无感的边界**：只读降级与来源标识保留。桌面端远程浏览仍按 `app/info` 的 `access`（viewer 级）做只读降级，前端无专门分支；浏览器插件的广域网保存使用独立的写会话，不改变桌面端远程浏览的只读语义；窗口保留「当前浏览：家里台式机」轻标识——这是安全与认知底线，不是可省略的区分逻辑

## 关键设计决策

**1. remote 是 daemon 内模块，不是独立进程**

原方案（独立进程 hawk-remote）的两个动机在当前前提下已失效或可用更省的方式满足：闭源云端边界（社区构建物理剔除云端代码）随服务端开源自托管而消失；崩溃域隔离在 Rust 里可用 supervisor 任务级重启达到同等实效——daemon 已有先例：LAN web 就是 reconcile 模型的可选网络子系统，故障只写状态不崩进程。合并后省掉三块接线：第三进程的 spawn/传参/回收、受托 token 双进程注入机制、独立状态 API 端口（设备列表/会话事件走 daemon 现有 REST + SSE）。代价是 quinn/igd 等依赖进入 daemon 依赖树（未启用时运行成本为零），以及 WAN 代码与素材库服务同进程（以 supervisor + 应用级远程配置可关闭缓解）。

**2. 直连优先，服务器只做撮合**

素材浏览是重带宽场景，自托管服务器的带宽/流量有限，数据面默认不经过服务器：服务端只做注册、在线与候选撮合（控制面），素材数据走设备间 QUIC 直连。中继（P2）仅作为直连失败时经用户确认的兜底，消耗的是用户自己的服务器带宽。

**3. 数据面从第一天用 QUIC（quinn）**

QUIC 自带多路复用、TLS 1.3 端到端加密、抗 NAT 重绑定，且为 P3 打洞（免端口映射 P2P）预留了传输基础——P3 已实现：Initial 自动重传天然覆盖洞口开启前的丢包，连接迁移容忍 NAT 重绑定。

**4. 协议契约先行，零代码共享**

客户端（daemon remote 模块）与 hawk-server 之间的契约是 remote-protocol.md。hawk-server 保持独立 crate、不建 workspace 羁绊、不共享代码——它对 hawk 的全部知识就是协议文档；remote 模块对 daemon 本体的全部知识就是进程内路由 + viewer 权限标记。

**5. 客户端最小知识：URL + KEY，无登录**

桌面端配置只有服务端地址 URL 与 service-key（应用级 `remote.toml`，跨素材库共享）。控制面无登录流程、无会话令牌；key 无效即 401，客户端直接展示服务端返回的 message。查看者凭证（hawk-app key）由目标设备 daemon 本地签发与保管（明文只在签发时出现一次，本地只存 sha256 验证子），server 经设备推送只存 HMAC 哈希映射；会话请求携带验证子，权限由目标 daemon 按本地映射执法（server 被攻破也无法放大权限）。

**6. 会话认证在 QUIC 层，隧道内 HTTP 进程内授权**

设备自签名证书指纹在服务端登记（TOFU），会话建立时经控制面随 `session.request` 下发对端指纹，QUIC 对端以指纹钉扎验证。隧道内 HTTP 由 A 侧隧道端在进程内标记权限：只读会话为 viewer 级；写会话需查看者 key 带写权限 + 目标设备 `remote.writable=true`，且仅开放导入素材/新建文件夹。查看方 B 全程不接触 daemon 的任何 token，服务端也接触不到 daemon admin token。

### 广域网写权限分层

| 层 | 控制项 | 作用 |
| -- | ------ | ---- |
| hawk-server | hawk-app key 映射（HMAC 哈希） | 只有映射中带写权限的 key 能换发写作用域 bt；key 由目标设备 daemon 推送，吊销即失效 |
| 目标设备 | `remote.writable` | 允许广域网写会话导入素材/新建文件夹；默认关闭，每请求校验，关闭立即失效 |
| 目标 daemon | 本地 key 映射执法 + `AccessLevel::RemoteWriter` 端点白名单 | 会话请求携带 key 验证子，本地裁决存在性与权限（server 声称的 access 不采信）；只放行 GET、查询类 POST、`item/add`（禁止 `path`）与 `folder/create`；其余写端点 403 |

局域网写权限 `[web]` 与以上链路完全独立：开启局域网写不会放开广域网写，反之亦然。

**7. 中继需用户主动开启**

连接顺序：IPv4 直连（UPnP）→ IPv6 直连 → 询问用户是否中继。中继消耗自己的服务器带宽，任何环节都不自动回落——「中继回退」是客户端设置项（默认关闭），开启后直连失败时弹确认，用户同意才建立中继会话。IPv6 直连是免映射、不经服务器的 P2P 路径；候选可达性由客户端实际连接尝试判定，服务端只负责收集上报、不下结论。

## 远程查看模式（没有本地素材库）

daemon 是双形态进程：`--library` 缺省即远程查看模式——不构建任何库子系统（索引/监听/缩略图/LAN），库端点一律 400 `NO_LIBRARY`，仅保留远程管理面（`/remote/*`）与会话代理；接入配置与设备身份在应用级 remote.toml/remote/（无库无库内回退位，`--remote-config` 必显式提供）。无库设备注册时 `allow_remote=false`、库名恒为「远程查看」（可被池内发现，但没有素材可服务）。

首次启动或本地库失效时，引导页提供「仅浏览远程素材库」：主进程直接拉起无库 daemon（不创建任何本地目录），前端经 `app/info.has_library=false` 进远程查看屏（复用设置面板的远程分区：填服务端/密钥 → 选设备 → 连接）。冷启动模式记忆为 config.toml 的 `remoteViewer`（开本地库即复位）；真实本地库从远程查看屏「打开本地素材库」或侧栏库下拉切换。

```text
无 libraryPath → SetupScreen
  ├─ 打开资源文件夹 → 系统目录选择框 → 本地库
  └─ 仅浏览远程素材库 → 拉起无库 daemon（不建目录）→ 远程查看屏
       → 填服务端/密钥 → 选设备 → 连接（连接描述符切换，boot 进主界面）
```

## 连接建立流程

### 直连模式（阶段 1）

```text
B 配置接入（应用级 remote.toml url + key）→ 拉取设备列表（A 在线 + 心跳携带可达性信息）
B 前端发起会话（mode=direct）→ 创建会话，经 WSS 推送给 A：session.request
A 侧自动接受（设备级开关 allow_remote 控制）→ 回 accept
服务端进入打洞编排（P3，探测口启用时）：双方 WSS 收到 session.probe，
  各自从 QUIC 端点同端口 socket 向服务端探测口报到
  → 到齐（无候选等 8s / 有候选短等 2s）→ ready：候选列表（IPv4 映射 → IPv6 → punch）+ 指纹，
    A 另得 punch_peer（B 的 observed endpoint）
  → B 依序尝试候选（直连 5s / punch 15s），A 同时向 punch_peer 开口（500ms × 20s），
    任一候选成功即建立 QUIC 连接（指纹钉扎）
  → 隧道就绪 → 主进程切换连接描述符 → B 前端无感重置为远端库
探测口未启用：有候选直接 ready；无候选即 unreachable。
候选全部失败（含对称型 NAT 打不通）→ 直连失败，走中继确认路径
```

直连失败（候选全部超时或无候选）时不自动回落中继：「中继回退」开关（默认关闭）已开启时，弹窗询问用户是否经中继继续（消耗自己的服务器带宽），确认后转入中继流程；未开启则提示「当前网络无法直连；如需经中继访问，可在设置中开启」。

### 中继模式（阶段 2，已实现）

直连失败（候选全部超时或无候选）时，「中继回退」开关已开启的用户经确认弹窗同意后，以 `mode=relay` 另建会话：双方各连一条服务端中继 WS，QUIC 经 quinn 自定义传输（AsyncUdpSocket over WS，一帧 = 一个数据报）在 A↔B 之间端到端建立——服务端只配对转发加密数据报，看不到素材内容。多租户方案下无票据/配额/用量上报，准入即本池 service-key。协议细节见 [remote-protocol.md](remote-protocol.md)「中继」一节。

## 安全模型

| 环节     | 措施 |
| -------- | ---- |
| 控制面   | HTTPS/WSS（TLS 由前置反代终结）；service-key 程序生成 256-bit 随机（`tenant add`），服务端只存 HMAC-SHA256(pepper, key)，恒定时间比较；key 一律走 Authorization 头，不进 URL（防反代日志泄露） |
| 凭证风控 | gate/login/控制面鉴权失败按来源 IP 计（10min/30 次 → 冷却 15min）；login 另按租户计（20 次 → 池冷却 15min 起指数增长封顶 24h，成功清零）；门令牌 5min TTL + 5 次尝试预算 |
| 数据面   | QUIC TLS 1.3 端到端加密，证书指纹经控制面交换 + 钉扎；服务端接触不到素材内容（直连不过服务端，中继只搬运密文） |
| 隧道端点 | 隧道内请求在进程内转发时标记 viewer 级或受限远程写权限；写会话必须携带与 QUIC 对端指纹匹配的会话标识，且目标 `remote.writable=true`。`GET /api/v1/app/token`（免鉴权返回主 token，Host 限定环回）在隧道端直接 404 |
| 本地防护 | B 侧会话代理：每次会话随机 token、响应无 CORS、环回监听 |
| 凭据存储 | 接入配置（url + service_key）存应用级 `~/.config/hawk/remote.toml`（与壳偏好 `config.toml` 分开，daemon 权威写入；明文，权限 0600）；设备身份（device_id、自签证书私钥）存同级 `remote/identity.json`、访问密钥验证子存同级 `remote/keys.json`（均 0600）。浏览器插件的 service-key 与访问密钥存 `browser.storage.local`，与本地 daemon token 同级 |
| 网页版   | **隧道在服务器终结**：素材明文过服务器（自托管信任前提，与「中继只见密文」刻意不同的取舍）。浏览器令牌 bt 随机换发、服务端存 sha256 哈希与授权（作用域 + 绑定设备 + 凭证）、可随时注销；service-key 与 hawk-app key 都只出现在登录请求体（服务端只见 service-key 与 key 的哈希输入，key 明文以哈希落盘验证后即弃） |
| 广域网写 | 带写权限的 hawk-app key + 目标设备开关双重准入 + daemon 本地执法；远程写只覆盖导入与建文件夹，禁止 `path` 导入（防读本机文件）；目标关闭开关或吊销 key 后，现有会话的下一个写请求即 403 |

## 分发形态

- hawk-server 位于本仓库 `hawk-server/`，随 AGPL 开源，自托管部署（单二进制 + CLI/env 配置，TLS 交反代）
- daemon 单二进制内含 remote 模块，无第三进程、无构建期门控；远程配置未启用则运行时该能力不存在（任务不 spawn、端口不绑定）
- app 远程 UI 以 daemon `/api/v1/remote/*` 状态为准：未配置时展示接入引导，无独立构建开关

## 对 hawk 仓库的改动清单

| 位置                       | 改动 |
| -------------------------- | ---- |
| `hawk-server/`             | 远程访问服务端（多租户：租户注册表 + key 映射 + 门令牌两步登录 + 风控；P1 控制面 + P2 中继） |
| `hawk-daemon/src/remote/`  | 新模块：信令客户端 / 可达性 / QUIC 隧道端 / B 侧会话代理 / 访问密钥保管（keys.rs），supervisor reconcile 管理 |
| `hawk-daemon` config       | 应用级 `remote.toml`（enabled/url/service_key/writable），保存即热生效（写开关不重启信令）；设备身份与 keys.json 同级 `remote/` |
| `hawk-daemon` 鉴权中间件   | 进程内 viewer / remote_writer 权限标记（供隧道转发请求使用），无 remote 语义、无门控 |
| `hawk-daemon` API          | `/api/v1/remote/*` 状态端点 + 访问密钥管理端点 + SSE remote 会话生命周期事件 |
| `hawk-app`（主进程 + 前端） | 远程 UI（接入配置/访问密钥管理/设备列表/会话流程）；连接描述符与 `connection-changed`；订阅 daemon SSE remote 事件驱动断线回切 |
| `~/.config/hawk/remote/`   | 应用级设备身份目录（device_id、自签证书私钥）与访问密钥验证子（keys.json），不参与素材库同步 |

## 分级落地计划

| 阶段 | 内容 | 验收标准 |
| ---- | ---- | -------- |
| 1a   | ~~hawk-server P1 控制面 + daemon remote/ 信令~~（已完成）：应用级远程配置、设备身份、注册/心跳/WSS 自动重连、状态 API | 设备配置后可注册上线、维持在线、接收并自动响应会话请求 |
| 1b   | ~~daemon 数据面~~（已完成：可达性 UPnP/IPv6、QUIC 双向指纹钉扎、A 侧隧道端进程内转发 TunneledPeer、B 侧会话代理、会话编排 API + SSE remote.session）+ app 远程 UI 与连接描述符 | 有公网 IPv4（UPnP）或全局 IPv6 的设备端到端远程查看可用；切换本地/远端库无感；断线自动回切 |
| 2    | ~~服务端中继 + 客户端中继逻辑 +「中继回退」开关与确认弹窗~~（已完成：relay WS 配对转发 + AsyncUdpSocket over WS + 回退确认） | 直连失败经用户确认走中继；QUIC 端到端语义不变（服务端只见密文） |
| 3    | ~~QUIC 打洞（免端口映射 P2P）~~（已完成：服务端 UDP 探测口观测 observed endpoint + 会话时编排 + 双方同端口开口；对称型 NAT 打不通时落中继，不投 NAT 类型检测与端口预测） | 无直连候选的设备间可直连；探测口不可达自动退化为 unreachable |

## remote/ 模块结构

```text
hawk-daemon/src/remote/
├── mod.rs          # supervisor（reconcile：remote.toml 变更 → 热重启信令）+ 状态快照
├── config.rs       # 应用级 remote.toml 读写（设备身份目录由配置位置派生）
├── identity.rs     # remote/identity.json（device_id、rcgen 自签证书与指纹）
├── keys.rs         # remote/keys.json：访问密钥（hawk-app key）签发/保管/吊销 + server 映射推送
├── signal.rs       # REST 客户端（注册/心跳/设备/会话/key 同步）+ WSS 通道 + 自动重连 + 查看者会话本地执法
├── tls.rs          # QUIC 双向指纹钉扎（rustls verifier：B 钉 A、A 放行已接受对端）
├── reachability.rs # 全局 IPv6 探测（UDP connect 出接口）+ UPnP 映射维护（igd 阻塞任务）
├── data.rs         # quinn 端点（自建 socket + try_clone 开口句柄，P3 同端口前提）、A 侧隧道端（hyper-over-stream 进程内转发 TunneledPeer）、B 侧会话代理、打洞开口/探测
└── session.rs      # B 侧会话编排：发起 → 轮询 ready → 候选依序尝试（含 punch）→ 代理；SSE remote.session
```

新增依赖：tokio-tungstenite（WSS）、reqwest（REST，rustls）、rcgen + sha2（自签证书与指纹）、quinn（QUIC）、igd-next（UPnP）、hyper + hyper-util（隧道内 HTTP/1.1 解析）。

数据面传输模型：一条 QUIC 双向流即一条 HTTP/1.1 连接——B 侧会话代理逐请求开流原样转发，A 侧经 hyper 解析后进程内调 API 路由。SSE/缩略图等流式响应由 HTTP/1.1 语义原生承载，两端都不解析素材数据。

## 相关文档

- [远程访问协议](remote-protocol.md)（接口契约，唯一共享物）
- [hawk-server 使用指南](hawk-server.md)（启动配置、接口、运维排查）
- [远程访问部署](remote-deployment.md)（编译、反代、TLS）
- [hawk-app 设计](../frontend/hawk-app.md)（进程模型、打包分发）

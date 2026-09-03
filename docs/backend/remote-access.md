# 远程访问设计

> 远程访问是可选的云能力：客户端侧为独立进程 **hawk-remote**（hawk 仓库内，随 AGPL 开源），与 hawk-daemon、hawk-app 以进程接线协作；云端服务为独立实现（hawk-server 仓库，闭源）。接口契约见 [remote-protocol.md](remote-protocol.md)。

## 目标与非目标

**目标**：在客户端配置接入信息（信令地址 + 访问密钥）后，查看自己其他在线设备，远程浏览对方设备上的素材库（只读）。

**非目标**（v1 不做）：

- 远程写入素材库（修改标签、导入素材等）——远程一律只读，写入仍在本机完成
- 多人协作 / 共享库给他人——设备只属于同一账号
- 移动端查看器——后续迭代，协议不为此预留特殊设计

## 总体架构

每台设备上运行三个进程：hawk-app（Electron 壳）、hawk-daemon（素材库服务）、hawk-remote（广域网客户端，可选）。远程查看时，两台设备各自的 hawk-remote 之间建立端到端 QUIC 隧道，素材数据始终由「素材库所在机」的 daemon 产出。

```text
        设备 A（素材库所在机）                      设备 B（查看方）
┌────────────────────────────┐          ┌────────────────────────────┐
│ hawk-app (Electron)        │          │ hawk-app (Electron)        │
│  前端 ──HTTP──► daemon      │          │  前端 ──HTTP──► 本地代理     │
│  主进程：拉起/传参/回收       │          │  主进程：拉起/传参/回收       │
│      │            │         │          │      │            │         │
│      ▼            ▼         │          │      ▼            ▼         │
│ hawk-daemon     hawk-remote │          │ hawk-daemon     hawk-remote │
│ 127.0.0.1:p1    │状态API p5 │          │ 127.0.0.1:p1   │状态API p5  │
│ 索引/扫描/缩略图 │A侧隧道端◄──┼──QUIC 隧道（端到端加密）──►►B侧本地代理p4
│ 多认一种受托token│           │          │                │            │
└─────────────────┼───────────┘          └────────────────┼────────────┘
                  │  HTTPS/WSS 控制面（注册/心跳/会话/WSS）  │
                  └───────────────┬───────────────────────┘
                                  ▼
                     hawk-server（hub 控制面 + relay 中继，闭源独立仓库）
```

端口默认值：daemon API 27371 / LAN web 27372 / QUIC UDP 27373 / B 侧本地代理 27374 / remote 状态 API 27375。所有本地端口由 Electron 主进程预选分配后经参数下发，进程本身不固定绑定。

## 组件职责

### 云端 hawk-server（闭源，独立仓库）

- 与 hawk 仓库**零代码共享**：不含任何 hawk 代码，只实现协议契约（hub 控制面 + relay 中继，已基本实现，见该仓库 `docs/architecture.md`）
- 账户服务与中继节点之间通过内网 + 共享密钥通信（用量上报、票据密钥分发）

### hawk-remote（客户端广域网进程，hawk 仓库内独立项目）

客户端侧全部远程能力的承载者，独立二进制独立进程：

- 信令客户端：设备注册、周期心跳（30s）、WSS 控制通道（自动重连）、设备列表、会话发起与响应
- 可达性：UPnP/NAT-PMP 端口映射（igd）、公网 IPv4 / 全局 IPv6 探测上报；首次启用时为 QUIC UDP 端口添加防火墙入站规则
- 数据面：QUIC 隧道端与本地代理
  - A 侧隧道端：将隧道内 HTTP 请求转发到 `127.0.0.1:<daemon端口>`，注入受托只读 token、拒绝转发 `/api/v1/app/token`、改写 Host 为非环回值（见安全模型）
  - B 侧本地代理：监听预选端口，每次会话随机 token，web 查看器以 `?token=` 携带，响应不带 CORS 头
- 本地状态 API（环回）：设备列表、在线状态、连接状态、会话请求与生命周期事件，供 app 前端与主进程消费
- 中继客户端（P2）：UDP 直连中继 / TCP 帧模式

运行形态约束：**无 `--remote` 参数即退出（退出码 2）**——进程不被拉起就不存在；凭据之外不做任何本地持久化配置。

### hawk-daemon（素材库服务，唯一调用点）

daemon 对远程**零感知**，唯一的调用点是鉴权中间件多认一种 token 源：

- env `HAWK_DELEGATE_TOKEN`（受托只读 token，权限与 LAN viewer 相同：只读）。Electron 启动 daemon 时注入；未注入则该能力不存在
- 该机制是通用的「额外 token 源」，代码与命名不带 remote 语义，**不需要任何编译期门控**——daemon 源码与构建产物中不存在云端代码

隧道转发的请求对 daemon 而言就是普通环回 HTTP 请求：daemon 查内存索引、读本地文件夹、出缩略图，与处理局域网浏览器请求无任何区别。

### hawk-app（Electron 壳，接线枢纽）

主进程是三个进程唯一的参数枢纽：

- 生成 token（admin token + 受托只读 token）、预选全部本地端口
- spawn / 回收 daemon（现状不变）与 hawk-remote（见下节条件）
- 维护「当前活动连接」（见连接模型），向渲染进程下发连接变更
- 订阅 remote 状态 API 的会话生命周期事件，驱动断线回切

远程 UI（接入配置、设备列表、会话流程）收敛在 `hawk-app/src/remote/` 目录，经构建期常量 `HAWK_REMOTE` 接入路由。设置项含「中继回退」开关（默认关闭——中继可能计入额度，仅用户主动开启后，直连失败才会询问是否走中继）。

## 进程接线

### 启动接线

| 目标 | 参数与环境变量 | 来源 |
| ---- | ---- | ---- |
| hawk-daemon | `--library <path> --port <p1> --web-dist <dir>`；env `HAWK_TOKEN`（admin）、`HAWK_DELEGATE_TOKEN`（受托只读） | Electron 生成 token、预选 p1（现状 + 受托 token） |
| hawk-remote | `--remote <base> --remote-key <key> --library <path> --daemon-port <p1> --api-port <p5> --proxy-port <p4>`；env `HAWK_DELEGATE_TOKEN`（与 daemon 同值） | base/KEY 读取自 app userData；端口沿用预选值 |

**remote 进程的拉起条件**：产物携带了 hawk-remote 二进制 **且** 用户配置过接入信息。两者任一不满足则该进程不存在——社区构建与未配置远程的官方构建中，运行时只有两个进程。

### 知识边界

| 进程 | 需要知道 | 明确不知道 |
| ---- | ---- | ---- |
| hawk-daemon | admin token + 受托只读 token（均在 env） | remote 进程存在、hub、KEY、隧道 |
| hawk-remote | daemon 端口 + 受托 token、hub URL + KEY、自身设备身份 | app 内部、daemon 内部 |
| hawk-app 主进程 | 所有进程的地址与 token（枢纽）、产物是否携带 remote | 协议细节 |

### 交互清单

| # | 调用方 → 被调方 | 方式 | 时机 |
| - | ---- | ---- | ---- |
| 1 | app 主进程 → daemon | spawn + env/CLI | 启动、切库（现状不变） |
| 2 | app 前端 → daemon | HTTP（`?token=`，现有路径不变） | 全部日常操作 |
| 3 | app 主进程 → remote | spawn + env/CLI | 仅产物携带且已配置接入 |
| 4 | app 前端/主进程 → remote | HTTP 调状态 API（27375，token 经 preload IPC 下发） | 设备列表、发起会话、接受/拒绝、连接状态 |
| 5 | remote → hawk-server | REST（注册/心跳/会话）+ WSS（推送、保活） | 常驻 |
| 6 | remote(A) → daemon | 环回 HTTP 转发（注入受托 token） | 仅会话进行中 |
| 7 | remote ↔ remote（跨设备） | QUIC 端到端（指纹钉扎；直连或经 relay） | 会话数据面，不经任何 hub/app/daemon |

## 数据流：daemon 为何不需要理解远程

remote 是「网络延长线」，两端都只做字节转发，不产生、不转换任何素材数据。素材数据的源头始终是素材库所在机的 daemon：

```text
B 浏览器 / B app 查看窗口
  │ ① 普通 HTTP 请求
  ▼
B 侧 hawk-remote 本地代理 ──② 原样塞进 QUIC 隧道──►
A 侧 hawk-remote 隧道端 ──③ 改 Host + 注入受托 token，其余不动──►
A 的 daemon ──④ 与局域网请求无区别地处理──► 响应原路返回
```

- B 侧的 daemon 与远程查看完全无关（照常服务 B 自己的库）；跨公网传输的请求/响应，收件方是浏览器，不是任何 daemon
- app 前端查看远端库不需要新数据通路：app 的窗口与 web 查看器是同一个构建产物，只是 API base 指向本地代理端口

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

- 本地库：`api_base` 指向 daemon 端口；远程库：指向 B 侧本地代理端口
- 前端收到 `connection-changed` 后执行**统一的切换重置**：abort 进行中的请求与 SSE → 清空内存 store → `GET /app/info` → 按返回重新注水。该流程是启动注水的泛化，只写一次
- **断线自动回切**：隧道断开、对端离线时，主进程经 remote 状态 API 获知，自动把当前连接切回本地库再通知前端。用户看到的是无声回到自己的库，而非远程白屏报错
- **无感的边界**：只读降级与来源标识保留。远程库经 `app/info` 的 `access`（viewer 级）自然触发现有只读降级，前端无专门分支；窗口保留「当前浏览：家里台式机」轻标识——这是安全与认知底线，不是可省略的区分逻辑

## 关键设计决策

**1. 独立进程，不是 daemon 内模块**

广域网连接（信令、WSS 重连、QUIC、UPnP、防火墙）是故障模式最密集的代码。独立进程使 daemon 的崩溃域不扩大：remote 出问题只影响远程功能，素材库服务不受影响。与 Tailscale（tailscaled）、Mullvad（mullvad-daemon）、Syncthing 同构：网络重活放常驻进程，壳只做拉起、传参、展示。

**2. 门控在打包层，不在代码层**

原方案（daemon 内 `remote` feature 门控）会导致 `#[cfg(feature = "remote")]` 在 daemon 源码中扩散。新形态下 daemon 的唯一调用点（受托 token）是无 remote 语义的通用能力，无需门控；产物裁剪收敛为两处打包决策——CI 是否编译/携带 `hawk-remote` 二进制，前端是否定义 `HAWK_REMOTE`。社区构建与官方构建共用同一份 daemon 产物。

**3. 数据面从第一天用 QUIC（quinn），P2P 与中继同一传输**

QUIC 自带多路复用、TLS 1.3 端到端加密、抗 NAT 重绑定。中继只逐包转发加密数据报，看不到素材内容；中继被攻破也无数据泄露。

**4. 协议契约先行，零代码共享**

hawk 与云端服务之间的契约是 remote-protocol.md，放在 hawk 仓库公开。hawk-remote 与 hawk-daemon 之间除响应信封等微小结构外不共享代码、不建 workspace 羁绊，保持「hawk-remote 只是协议客户端」的独立性——它对 daemon 的全部知识就是 `127.0.0.1:<port>` + 受托 token。

**5. 客户端最小知识：URL + KEY，无登录**

客户端配置只有信令地址 URL 与访问密钥 KEY。协议无登录端点、无会话令牌；客户端对所有错误的处理一致：直接展示服务端返回的 message。详见协议文档「HTTP 状态码」一节。

**6. 会话认证在 QUIC 层，隧道内 HTTP 无鉴权中间人**

设备自签名证书指纹在信令服务登记（TOFU），会话建立时经信令交换指纹，QUIC 对端以指纹钉扎验证。隧道内 HTTP 由 A 侧隧道端统一注入受托只读 token——查看方 B 全程不接触 daemon 的任何 token，云端也接触不到。

**7. 直连优先，中继需用户主动开启**

连接顺序：IPv4 直连（UPnP）→ IPv6 直连 → 询问用户是否中继。中继消耗服务端带宽且可能计入用户额度，任何环节都不自动回落——「中继回退」是客户端设置项（默认关闭），开启后直连失败时弹确认，用户同意才建立中继会话。IPv6 直连是免映射、不计量的 P2P 路径；候选可达性由客户端实际连接尝试判定，信令服务只负责收集上报、不下结论。

## 连接建立流程

### 直连模式（阶段 1）

```text
B 配置接入 → 拉取设备列表（A 在线 + 心跳携带可达性信息）
B 前端经 remote 状态API 发起会话（mode=direct）
hawk-server：A 心跳上报过可达候选（IPv4 UPnP 映射 / 全局 IPv6）？
  ├─ 是 → 创建会话，经 WSS 推送给 A：session.request
  │        A 侧 remote 自动接受（设备级开关 allow_remote 控制）→ 回 accept
  │        服务端向 B 返回候选列表 connect.candidates（IPv4 映射优先、IPv6 次之）+ 指纹
  │        B 依序尝试候选（单候选超时 5s），任一成功即建立 QUIC 连接（指纹钉扎）
  │        隧道就绪 → 主进程切换连接描述符 → B 前端无感重置为远端库
  └─ 无任何候选 → 会话状态 unreachable
```

直连失败（候选全部超时或无候选）时不自动回落中继：「中继回退」开关（默认关闭）已开启时，弹窗询问用户是否经中继继续（中继经服务端计量，可能消耗额度），确认后转入中继流程；未开启则提示「当前网络无法直连；如需经中继访问，可在设置中开启」。

### 中继模式（阶段 2）

```text
B 发起会话（mode=relay，仅在直连失败且用户于确认弹窗中同意后发起）
hawk-server：校验访问资格与中继额度（服务端规则，拒绝时 401）→ 签发两张中继票据（role=a/b，含配额、有效期）
双方经各自 WSS 通道收到票据 → 各自连接中继节点（UDP；UDP 被封锁时 TCP 帧模式）
中继验签 → 配对双方地址 → 逐包转发（QUIC 在 A↔B 之间端到端建立，中继只搬运）
会话期间中继计量字节数 → 周期上报账户服务 → 额度用尽断流
```

## 安全模型

| 环节 | 措施 |
| ---- | ---- |
| 控制面 | HTTPS/WSS，access_key 为 256-bit 随机值，服务端只存哈希 |
| 数据面 | QUIC TLS 1.3 端到端加密，证书指纹经信令交换 + 钉扎；中继不可读内容 |
| 隧道端点 | A 侧隧道端拒绝转发 `GET /api/v1/app/token`（404），并统一改写转发请求 Host 为非环回值（如 `hawk-remote.tunnel`）——该端点免鉴权返回主 token，Host 改写使其自带的环回检查成为第二道闸（纵深防御，见协议文档数据面一节） |
| 本地防护 | 受托 token 只存在于 daemon 与 remote 的进程 env（不落盘）；B 侧代理会话级随机 token、响应无 CORS；remote 状态 API 限定环回 + token |
| 中继票据 | HMAC 签名，绑定会话与配额，短有效期（建立窗口 10 分钟），防重放靠 source addr 绑定 |
| 凭据存储 | 接入配置（URL + KEY）存 app userData（账号级，随账号不随库）；设备身份（device_id、自签证书私钥）存 `.hawk/remote/`（库级，本地专用目录，不参与同步） |

## 分发与开源边界

- hawk-remote 位于 hawk 仓库内，随 AGPL 开源——分发含它的二进制本就要求提供对应源码，放同一仓库是最直接的履行方式
- 产物裁剪是打包决策：CI 双矩阵——社区构建 `cargo build -p hawk-daemon`（不编 hawk-remote）+ 前端 `HAWK_REMOTE=false`，产物中物理上不存在云端连接代码与 UI；官方构建额外编译 hawk-remote 并随 electron-builder `extraResources` 携带 + `HAWK_REMOTE=true`
- CI 增加产物清洁性检查（社区构建中搜不到 remote 标识字符串）
- 云端服务闭源、只部署在自己的基础设施上，从不随客户端分发

## 对 hawk 仓库的改动清单

| 位置 | 改动 |
| ---- | ---- |
| `hawk-remote/` | 新项目（独立 crate）：信令客户端 / WSS 通道 / UPnP / QUIC 隧道端 / B 侧本地代理 / 本地状态 API |
| `hawk-daemon/src/api/mod.rs` | 鉴权中间件增加受托 token 源（env `HAWK_DELEGATE_TOKEN`，只读，与 LAN viewer 同级）。daemon 其余零改动、零门控 |
| `hawk-app/electron/`（主进程模块） | 端口统一预选；受托 token 生成与双进程注入；hawk-remote 拉起/回收/崩溃重启；连接描述符与 `connection-changed`；订阅 remote 状态 API |
| `hawk-app/src/remote/` | 接入配置（URL + KEY）/ 设备列表 / 连接流程 / 错误提示 UI，构建期 `HAWK_REMOTE` 门控 |
| `hawk-app` web 前端 | 连接参数由启动时常量改为响应式描述符；切换重置流程（启动注水逻辑泛化） |
| `.hawk/remote/` | 本地专用设备身份目录（device_id、自签证书），不参与同步 |
| app userData | 接入配置（URL + KEY） |

## 分级落地计划

| 阶段 | 内容 | 验收标准 |
| ---- | ---- | ---- |
| 1 | hawk-remote（注册/心跳/WSS/直连：IPv4 UPnP + IPv6、QUIC 隧道端、B 侧代理、状态 API）+ daemon 受托 token + app 接线与连接描述符 + 双构建体系 | 有公网 IPv4（UPnP）或全局 IPv6 的设备端到端远程查看可用；app 内切换本地/远端库无感；断线自动回切；社区构建产物无云端代码；401 访问控制可用 |
| 2 | 中继客户端（UDP 直连 / TCP 帧模式）+「中继回退」开关与确认弹窗 | 直连失败经用户确认走中继；额度计量与超配额断流正确（服务端已在 hawk-server 实现） |
| 3 | QUIC 打洞（免端口映射 P2P，心跳已预留 `nat_type`） | 视阶段 1 直连成功率数据决定是否投入 |

## hawk-remote 代码结构（规划）

```text
hawk-remote/
├── Cargo.toml          # 独立 crate，不与 hawk-daemon 同 workspace
└── src/
    ├── main.rs         # CLI 解析；无 --remote 参数退出（码 2）
    ├── settings.rs     # --remote <base> --remote-key <key> --library <path>
    │                   # --daemon-port <p1> --api-port <p5> --proxy-port <p4>
    ├── store.rs        # .hawk/remote/（device_id、证书私钥）
    ├── signal/         # REST 客户端（注册/心跳/设备/会话）+ WSS 通道 + 自动重连
    ├── reachability/   # 公网 v4/v6 探测、UPnP/NAT-PMP 映射维护、防火墙规则
    ├── session/        # 会话编排、候选依序尝试（单候选 5s）
    ├── data/           # quinn 端点/自签证书、A 侧隧道端、B 侧本地代理
    └── api/            # 本地状态 API（环回，供前端与主进程）
```

主要依赖：tokio、axum（状态 API 与本地代理）、quinn + rcgen（QUIC 与自签证书）、igd（UPnP/NAT-PMP）、tokio-tungstenite（WSS）、reqwest 或 ureq（REST）。

## 相关文档

- [远程访问协议](remote-protocol.md)（接口契约，唯一共享物）
- hawk-server 仓库 `docs/architecture.md`（hub/relay 实现架构）
- [hawk-app 设计](../frontend/hawk-app.md)（进程模型、打包分发）
